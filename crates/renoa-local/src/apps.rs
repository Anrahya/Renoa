//! Supervised, `bubblewrap`-sandboxed live web apps served via the site gateway.

pub(crate) mod health;
mod model;
mod reconcile;
mod store;
mod supervisor;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::sites::{
    DirCleanup, create_private_dir, ensure_state_subdir, resolve_source_folder, validate_site_name,
};
use model::{APPS_RELATIVE_SUBDIR, LOG_FILE_NAME, SCRATCH_DIR_NAME};
use reconcile::{ensure_runtime_dirs, log_file_path, now_ms};
use store::{
    AppRecord, ensure_reachable, insert_new_app, list_records, read_record, set_state_failed,
    set_state_stopped, update_existing_on_failure, update_existing_on_start, update_last_active,
    used_ports,
};
use supervisor::UnitStatus;

pub use model::{AppError, AppLogs, AppState, IDLE_TIMEOUT, LiveApp, STARTUP_TIMEOUT};
pub(crate) use model::{
    CPU_QUOTA, CRASH_BURST, CRASH_INTERVAL_SECS, LOG_TAIL_BYTES, LOG_TAIL_LINES, MEMORY_MAX,
};
pub(crate) use store::initialize;
#[cfg(test)]
pub(crate) use store::restore_schema_55;
pub(crate) use supervisor::stop_units_sync;
#[cfg(test)]
pub(crate) use supervisor::{stop_unit_sync, unit_main_pid_sync};

/// Host-owned registry and supervisor bridge for live web apps.
#[derive(Debug, Clone)]
pub struct Apps {
    database: PathBuf,
    idle_timeout: Duration,
    startup_timeout: Duration,
    locks: Arc<StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

struct StartSpec {
    agent: AgentId,
    principal: Option<Uuid>,
    name: String,
    source_folder: String,
    command: String,
    app_dir: PathBuf,
}

impl Apps {
    #[must_use]
    pub fn open(home: &RenoaHome) -> Self {
        Self::from_database(&home.host_database())
    }

    #[must_use]
    pub fn from_database(database: &Path) -> Self {
        Self {
            database: database.to_path_buf(),
            idle_timeout: IDLE_TIMEOUT,
            startup_timeout: STARTUP_TIMEOUT,
            locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_timeouts(mut self, idle_timeout: Duration, startup_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self.startup_timeout = startup_timeout;
        self
    }

    #[must_use]
    pub const fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    fn lock_for(&self, name: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            map.entry(name.to_owned())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }

    /// Validates, sandboxes, starts, and registers a live app. A failed
    /// initial registration leaves no unit, scratch directory, or catalog row.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name/folder/command, ownership mismatch,
    /// missing supervisor (`bwrap` / `systemd --user`), non-loopback bind, or
    /// startup failure.
    pub async fn start(
        &self,
        agent: AgentId,
        principal: Option<Uuid>,
        workspace: &Path,
        name: &str,
        folder: &str,
        command: &str,
    ) -> Result<LiveApp, AppError> {
        let name = validate_site_name(name)?;
        let command = validate_command(command)?;
        let (app_dir, _) = resolve_source_folder(workspace, folder)?;
        let source_folder = app_dir
            .to_str()
            .ok_or_else(|| AppError::InvalidFolder("folder path is not UTF-8".into()))?
            .to_owned();
        let spec = StartSpec {
            agent,
            principal,
            name,
            source_folder,
            command,
            app_dir,
        };

        let mutex = self.lock_for(&spec.name);
        let _guard = mutex.lock().await;

        let existing = read_record(&self.database, &spec.name)?;
        if let Some(rec) = &existing {
            ensure_reachable(rec, spec.principal)?;
            if let Some(reused) = self.try_reuse_running(&spec, rec).await? {
                return Ok(reused);
            }
        }

        supervisor::preflight().await?;
        let apps_root = ensure_state_subdir(&self.database, APPS_RELATIVE_SUBDIR)?;

        if let Some(rec) = existing {
            return self.restart_existing_on_start(spec, &rec, &apps_root).await;
        }
        self.create_and_start_app(spec, &apps_root).await
    }

    async fn try_reuse_running(
        &self,
        spec: &StartSpec,
        rec: &AppRecord,
    ) -> Result<Option<LiveApp>, AppError> {
        if rec.app.state != AppState::Running
            || rec.app.source_folder != spec.source_folder
            || rec.app.command != spec.command
        {
            return Ok(None);
        }
        let unit = supervisor::unit_name(&self.database, &spec.name);
        if !matches!(supervisor::unit_status(&unit).await, Ok(UnitStatus::Active)) {
            return Ok(None);
        }
        match self.verify_live_loopback_http(rec).await {
            Ok(true) => {
                let now = now_ms()?;
                update_last_active(&self.database, &spec.name, now)?;
                let mut app = rec.app.clone();
                app.last_active_ms = now;
                Ok(Some(app))
            }
            Ok(false) => Ok(None),
            Err(reason) => Err(AppError::StartupFailed { reason }),
        }
    }

    async fn restart_existing_on_start(
        &self,
        spec: StartSpec,
        rec: &AppRecord,
        apps_root: &Path,
    ) -> Result<LiveApp, AppError> {
        let scratch = ensure_runtime_dirs(apps_root, &rec.app_id)?;
        let log_file = apps_root.join(&rec.app_id).join(LOG_FILE_NAME);
        let agent_str = spec.agent.to_string();
        let principal_str = spec.principal.map(|u| u.to_string());
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &rec.app.name);
        if let Err(reason) = self
            .boot_unit(
                &rec.app.name,
                spec.app_dir,
                scratch,
                log_file,
                spec.command.clone(),
                rec.app.port,
            )
            .await
        {
            let _ = update_existing_on_failure(
                &self.database,
                &rec.app.name,
                &agent_str,
                principal_str.as_deref(),
                &spec.source_folder,
                &spec.command,
                &reason,
                now_ms()?,
            );
            return Err(AppError::StartupFailed { reason });
        }
        update_existing_on_start(
            &self.database,
            &rec.app.name,
            &agent_str,
            principal_str.as_deref(),
            &spec.source_folder,
            &spec.command,
            now_ms()?,
        )?;
        unit_cleanup.disarm();
        read_record(&self.database, &rec.app.name)?
            .map(|r| r.app)
            .ok_or(AppError::NotFound)
    }

    async fn create_and_start_app(
        &self,
        spec: StartSpec,
        apps_root: &Path,
    ) -> Result<LiveApp, AppError> {
        let app_id = Uuid::new_v4().to_string();
        let runtime_dir = apps_root.join(&app_id);
        let mut cleanup = DirCleanup::armed(runtime_dir.clone());
        create_private_dir(&runtime_dir)?;
        let scratch = runtime_dir.join(SCRATCH_DIR_NAME);
        create_private_dir(&scratch)?;
        let log_file = runtime_dir.join(LOG_FILE_NAME);

        let port_reservation = health::allocate_port(&used_ports(&self.database)?, &spec.name)?;
        let port = port_reservation.port();
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &spec.name);
        if let Err(reason) = self
            .boot_unit(
                &spec.name,
                spec.app_dir,
                scratch,
                log_file,
                spec.command.clone(),
                port,
            )
            .await
        {
            return Err(AppError::StartupFailed { reason });
        }

        let now = now_ms()?;
        let principal_str = spec.principal.map(|u| u.to_string());
        let record = LiveApp {
            name: spec.name,
            agent_id: spec.agent.to_string(),
            source_folder: spec.source_folder,
            command: spec.command,
            port,
            state: AppState::Running,
            failure_reason: None,
            updated_at_ms: now,
            last_active_ms: now,
        };
        insert_new_app(&self.database, &record, principal_str.as_deref(), &app_id)?;
        unit_cleanup.disarm();
        cleanup.disarm();
        drop(port_reservation);
        Ok(record)
    }

    /// Stops a running app for `principal`.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name, unknown app, ownership mismatch, or
    /// catalog/systemd failure.
    pub async fn stop_for(&self, name: &str, principal: Option<Uuid>) -> Result<LiveApp, AppError> {
        let name = validate_site_name(name)?;
        let mutex = self.lock_for(&name);
        let _guard = mutex.lock().await;

        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        ensure_reachable(&rec, principal)?;
        self.stop_locked(&rec).await
    }

    async fn stop_locked(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        supervisor::stop_unit(&supervisor::unit_name(&self.database, &rec.app.name)).await?;
        set_state_stopped(&self.database, &rec.app.name, now_ms()?)?;
        read_record(&self.database, &rec.app.name)?
            .map(|r| r.app)
            .ok_or(AppError::NotFound)
    }

    /// Restarts an existing app for `principal`.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name, unknown app, ownership mismatch,
    /// invalid folder, or startup failure.
    pub async fn restart(&self, name: &str, principal: Option<Uuid>) -> Result<LiveApp, AppError> {
        let name = validate_site_name(name)?;
        let mutex = self.lock_for(&name);
        let _guard = mutex.lock().await;

        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        ensure_reachable(&rec, principal)?;
        self.launch_existing(&rec).await
    }

    /// Reads one app's current live state, reconciling against `systemd`.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name, unknown app, ownership mismatch, or
    /// catalog failure.
    pub async fn status_for(
        &self,
        name: &str,
        principal: Option<Uuid>,
    ) -> Result<LiveApp, AppError> {
        let name = validate_site_name(name)?;
        let mutex = self.lock_for(&name);
        let _guard = mutex.lock().await;

        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        ensure_reachable(&rec, principal)?;
        self.sync_live_state(&rec).await
    }

    /// Lists all apps reachable by `principal`, synced against `systemd`.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog failure.
    pub async fn list(&self, principal: Option<Uuid>) -> Result<Vec<LiveApp>, AppError> {
        let records = list_records(&self.database, principal)?;
        let mut out = Vec::with_capacity(records.len());
        for rec in records {
            let mutex = self.lock_for(&rec.app.name);
            let _guard = mutex.lock().await;
            if let Some(current) = read_record(&self.database, &rec.app.name)? {
                out.push(self.sync_live_state(&current).await?);
            }
        }
        Ok(out)
    }

    /// Returns the app's synced status and tail of `app.log`.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name, unknown app, ownership mismatch, or
    /// catalog failure.
    pub async fn logs_for(&self, name: &str, principal: Option<Uuid>) -> Result<AppLogs, AppError> {
        let name = validate_site_name(name)?;
        let mutex = self.lock_for(&name);
        let _guard = mutex.lock().await;

        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        ensure_reachable(&rec, principal)?;
        let app = self.sync_live_state(&rec).await?;
        let logs = supervisor::read_log_tail(&log_file_path(&self.database, &rec.app_id));
        Ok(AppLogs { app, logs })
    }

    /// Checks whether a valid app name exists in `host_apps`.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog failure.
    pub fn exists(&self, name: &str) -> Result<bool, AppError> {
        let Ok(name) = validate_site_name(name) else {
            return Ok(false);
        };
        Ok(read_record(&self.database, &name)?.is_some())
    }

    /// Ensures a registered app is running and healthy before proxying an HTTP
    /// request. Wakes `stopped` apps once; refuses `failed` apps immediately.
    ///
    /// # Errors
    /// Returns [`AppError`] if the app does not exist, previously failed, fails to
    /// wake cleanly, or is unreachable on its port.
    pub async fn ensure_running(&self, name: &str) -> Result<LiveApp, AppError> {
        let name = validate_site_name(name)?;
        let mutex = self.lock_for(&name);
        let _guard = mutex.lock().await;

        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        match rec.app.state {
            AppState::Failed => Err(self.failed_error(&rec)),
            AppState::Stopped => self.launch_existing(&rec).await.map_err(|err| match err {
                AppError::StartupFailed { reason } => AppError::Failed {
                    name: rec.app.name.clone(),
                    reason,
                    logs: supervisor::read_log_tail(&log_file_path(&self.database, &rec.app_id)),
                },
                other => other,
            }),
            AppState::Running => {
                let synced = self.sync_live_state(&rec).await?;
                if synced.state == AppState::Failed {
                    let latest = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
                    return Err(self.failed_error(&latest));
                }
                let log_file = log_file_path(&self.database, &rec.app_id);
                match self.verify_live_loopback_http(&rec).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let unit = supervisor::unit_name(&self.database, &rec.app.name);
                        if let Err(reason) = health::wait_until_healthy(
                            &unit,
                            synced.port,
                            &log_file,
                            self.startup_timeout,
                        )
                        .await
                        {
                            let _ = supervisor::stop_unit(&unit).await;
                            let _ =
                                set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?);
                            return Err(AppError::Failed {
                                name: rec.app.name.clone(),
                                reason,
                                logs: supervisor::read_log_tail(&log_file),
                            });
                        }
                    }
                    Err(reason) => {
                        return Err(AppError::Failed {
                            name: rec.app.name.clone(),
                            reason,
                            logs: supervisor::read_log_tail(&log_file),
                        });
                    }
                }
                let now = now_ms()?;
                update_last_active(&self.database, &synced.name, now)?;
                let mut active = synced;
                active.last_active_ms = now;
                Ok(active)
            }
        }
    }

    /// Updates `last_active_ms` for an active running app.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog failure.
    pub fn touch(&self, name: &str) -> Result<(), AppError> {
        let Ok(name) = validate_site_name(name) else {
            return Ok(());
        };
        update_last_active(&self.database, &name, now_ms()?)
    }
}

fn validate_command(command: &str) -> Result<String, AppError> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(AppError::InvalidCommand);
    }
    Ok(trimmed.to_owned())
}
