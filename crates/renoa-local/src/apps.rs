//! Supervised, `bubblewrap`-sandboxed live web apps served via the site gateway.

pub(crate) mod health;
mod launch;
mod model;
mod reconcile;
mod sandbox;
mod store;
mod supervisor;
#[cfg(test)]
mod tests;
mod unit_names;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as StdMutex},
    time::Duration,
};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::sites::{resolve_source_folder, validate_site_name};
use health::StartupError;
use launch::{ensure_apps_root, log_file_path, now_ms};
use store::{
    AppRecord, ensure_reachable, list_records, read_record, set_state_failed, set_state_stopped,
    update_last_active,
};
use supervisor::UnitStatus;

pub use model::{
    AppError, AppLogs, AppState, IDLE_TIMEOUT, IDLE_TIMEOUT_MINUTES, LiveApp, STARTUP_TIMEOUT,
};
pub(crate) use model::{CPU_QUOTA, CRASH_BURST, CRASH_INTERVAL_SECS, MEMORY_MAX};
pub(crate) use store::initialize;
#[cfg(test)]
pub(crate) use store::restore_schema_55;
pub(crate) use supervisor::stop_units_sync;
#[cfg(test)]
pub(crate) use supervisor::{stop_unit_sync, unit_main_pid_sync};

/// Keeps an app marked busy while an HTTP response or WebSocket stream is open
/// so idle sweeping and capacity eviction do not interrupt active transfers.
#[derive(Debug)]
pub struct AppStreamGuard {
    name: String,
    database: PathBuf,
    active_streams: Arc<StdMutex<HashMap<String, usize>>>,
}

impl Drop for AppStreamGuard {
    fn drop(&mut self) {
        {
            let mut guard = self
                .active_streams
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(count) = guard.get_mut(&self.name) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guard.remove(&self.name);
                }
            }
        }
        if let Ok(now) = now_ms() {
            let _ = update_last_active(&self.database, &self.name, now);
        }
    }
}

type AppLocks = Arc<StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>>;
type StreamCounts = Arc<StdMutex<HashMap<String, usize>>>;

/// Per-app locks and open-stream counts for each Host database. The agent
/// tool, the control API, and the gateway each hold their own `Apps`; sharing
/// this state makes them serialize on one app and see each other's streams.
static SHARED_STATE: LazyLock<StdMutex<HashMap<PathBuf, (AppLocks, StreamCounts)>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Host-owned registry and supervisor bridge for live web apps.
#[derive(Debug, Clone)]
pub struct Apps {
    database: PathBuf,
    idle_timeout: Duration,
    startup_timeout: Duration,
    locks: AppLocks,
    active_streams: StreamCounts,
}

struct StartSpec {
    pub agent: AgentId,
    pub principal: Option<Uuid>,
    pub name: String,
    pub source_folder: String,
    pub command: String,
    pub app_dir: PathBuf,
}

impl Apps {
    #[must_use]
    pub fn open(home: &RenoaHome) -> Self {
        Self::from_database(&home.host_database())
    }

    #[must_use]
    pub fn from_database(database: &Path) -> Self {
        let key = std::fs::canonicalize(database).unwrap_or_else(|_| database.to_path_buf());
        let (locks, active_streams) = SHARED_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key)
            .or_default()
            .clone();
        Self {
            database: database.to_path_buf(),
            idle_timeout: IDLE_TIMEOUT,
            startup_timeout: STARTUP_TIMEOUT,
            locks,
            active_streams,
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

    /// Acquires an active-stream guard for `name`, preventing idle/capacity eviction
    /// while held and refreshing `last_active_ms` on drop.
    #[must_use]
    pub fn acquire_stream(&self, name: &str) -> AppStreamGuard {
        {
            let mut guard = self
                .active_streams
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = guard.entry(name.to_owned()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        AppStreamGuard {
            name: name.to_owned(),
            database: self.database.clone(),
            active_streams: Arc::clone(&self.active_streams),
        }
    }

    fn active_stream_count(&self, name: &str) -> usize {
        self.active_streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    fn lock_for(&self, name: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|k, v| k == name || Arc::strong_count(v) > 1);
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
    /// missing supervisor (`bwrap` / `slirp4netns` / `systemd --user`),
    /// non-loopback bind, or startup failure.
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
        let (app_dir, normalized) = resolve_source_folder(workspace, folder)?;
        if normalized == "." {
            return Err(AppError::InvalidFolder(
                "folder must name a subdirectory inside the workspace, not `.`".into(),
            ));
        }
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
        let apps_root = ensure_apps_root(&self.database)?;

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
        let unit = unit_names::unit_name(&self.database, &spec.name);
        if !matches!(
            supervisor::unit_status(&unit).await,
            Ok(UnitStatus::Active { .. })
        ) {
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
        supervisor::stop_unit(&unit_names::unit_name(&self.database, &rec.app.name)).await?;
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
    /// catalog/systemd failure.
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
    /// Returns [`AppError`] on catalog or `systemd` failure.
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
    /// catalog/systemd failure.
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

    /// Fast-path lookup for the site gateway: returns the assigned loopback
    /// port if SQLite marks `name` `Running`, `None` if `Stopped`, or `Err` if
    /// `Failed` or unknown. Avoids `systemctl`, `/proc`, and HTTP probes.
    ///
    /// # Errors
    /// Returns [`AppError`] on invalid name, unknown app, failed app state, or
    /// catalog failure.
    pub fn running_port(&self, name: &str) -> Result<Option<u16>, AppError> {
        let name = validate_site_name(name)?;
        let rec = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
        match rec.app.state {
            AppState::Running => {
                let _ = update_last_active(&self.database, &name, now_ms()?);
                Ok(Some(rec.app.port))
            }
            AppState::Stopped => Ok(None),
            AppState::Failed => Err(self.failed_error(&rec)),
        }
    }

    /// Ensures a registered app is running and healthy before proxying an HTTP
    /// or WebSocket request. Wakes `stopped` apps once; refuses `failed` apps.
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
            AppState::Stopped => self.wake_existing(&rec).await,
            AppState::Running => {
                let synced = self.sync_live_state(&rec).await?;
                if synced.state == AppState::Failed {
                    let latest = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
                    return Err(self.failed_error(&latest));
                }
                if synced.state == AppState::Stopped {
                    let latest = read_record(&self.database, &name)?.ok_or(AppError::NotFound)?;
                    return self.wake_existing(&latest).await;
                }
                let log_file = log_file_path(&self.database, &rec.app_id);
                match self.verify_live_loopback_http(&rec).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let unit = unit_names::unit_name(&self.database, &rec.app.name);
                        if let Err(err) = health::wait_until_healthy(
                            &unit,
                            synced.port,
                            &log_file,
                            self.startup_timeout,
                        )
                        .await
                        {
                            let _ = supervisor::stop_unit(&unit).await;
                            return match err {
                                StartupError::TimedOut(reason) => {
                                    let _ =
                                        set_state_stopped(&self.database, &rec.app.name, now_ms()?);
                                    Err(AppError::StartupFailed { reason })
                                }
                                StartupError::Failed(reason) => {
                                    let _ = set_state_failed(
                                        &self.database,
                                        &rec.app.name,
                                        &reason,
                                        now_ms()?,
                                    );
                                    Err(AppError::Failed {
                                        name: rec.app.name.clone(),
                                        reason,
                                        logs: supervisor::read_log_tail(&log_file),
                                    })
                                }
                            };
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
