//! App process launch, staging validation, runtime directory setup, and clock helpers.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_home::APPS_DIRECTORY_PATH;
use uuid::Uuid;

use super::{
    AppError, AppState, Apps, LiveApp, StartSpec, health,
    health::StartupError,
    model::{LOG_FILE_NAME, SCRATCH_DIR_NAME},
    store::{
        AppRecord, insert_new_app, read_record, set_state_failed, set_state_running,
        set_state_stopped, update_existing_on_start, used_ports,
    },
    supervisor::{self, StartUnitSpec, UnitStatus},
};
use crate::sites::{DirCleanup, create_private_dir};

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

impl Apps {
    pub(super) async fn restart_existing_on_start(
        &self,
        spec: StartSpec,
        rec: &AppRecord,
        apps_root: &Path,
    ) -> Result<LiveApp, AppError> {
        let unit = supervisor::unit_name(&self.database, &rec.app.name);
        let config_changed =
            rec.app.source_folder != spec.source_folder || rec.app.command != spec.command;
        let currently_live = rec.app.state == AppState::Running
            && matches!(
                supervisor::unit_status(&unit).await,
                Ok(UnitStatus::Active { .. } | UnitStatus::Activating)
            );

        if config_changed && currently_live {
            if let Err(reason) = self.boot_staging_unit(&spec, apps_root).await {
                return Err(AppError::StartupFailed { reason });
            }
        } else if rec.app.state != AppState::Running {
            self.enforce_running_cap(Some(&rec.app.name)).await?;
        }

        let (runtime_dir, scratch_dir) = ensure_runtime_dirs(apps_root, &rec.app_id)?;
        let log_file = runtime_dir.join(LOG_FILE_NAME);
        let agent_str = spec.agent.to_string();
        let principal_str = spec.principal.map(|u| u.to_string());
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &rec.app.name);
        let unit_spec = StartUnitSpec {
            unit,
            app_name: rec.app.name.clone(),
            app_dir: spec.app_dir,
            scratch_dir,
            runtime_dir,
            log_file,
            command: spec.command.clone(),
            port: rec.app.port,
            base_path: format!("/apps/{}", rec.app.name),
        };
        match self.boot_unit(&unit_spec).await {
            Ok(()) => {}
            Err(StartupError::TimedOut(reason)) => {
                unit_cleanup.cleanup().await;
                let _ = set_state_stopped(&self.database, &rec.app.name, now_ms()?);
                return Err(AppError::StartupFailed { reason });
            }
            Err(StartupError::Failed(reason)) => {
                unit_cleanup.cleanup().await;
                let _ = set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?);
                return Err(AppError::StartupFailed { reason });
            }
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

    pub(super) async fn create_and_start_app(
        &self,
        spec: StartSpec,
        apps_root: &Path,
    ) -> Result<LiveApp, AppError> {
        self.enforce_running_cap(None).await?;

        let app_id = Uuid::new_v4().to_string();
        let raw_runtime_dir = apps_root.join(&app_id);
        let mut cleanup = DirCleanup::armed(raw_runtime_dir.clone());
        create_private_dir(&raw_runtime_dir)?;
        let raw_scratch = raw_runtime_dir.join(SCRATCH_DIR_NAME);
        create_private_dir(&raw_scratch)?;
        let runtime_dir = fs::canonicalize(&raw_runtime_dir)?;
        let scratch_dir = fs::canonicalize(&raw_scratch)?;
        let log_file = runtime_dir.join(LOG_FILE_NAME);

        let port_reservation = health::allocate_port(&used_ports(&self.database)?, &spec.name)?;
        let port = port_reservation.port();
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &spec.name);
        let unit_spec = StartUnitSpec {
            unit: supervisor::unit_name(&self.database, &spec.name),
            app_name: spec.name.clone(),
            app_dir: spec.app_dir,
            scratch_dir,
            runtime_dir,
            log_file,
            command: spec.command.clone(),
            port,
            base_path: format!("/apps/{}", spec.name),
        };
        if let Err(err) = self.boot_unit(&unit_spec).await {
            unit_cleanup.cleanup().await;
            return Err(AppError::StartupFailed {
                reason: err.into_message(),
            });
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
        if let Err(err) = insert_new_app(&self.database, &record, principal_str.as_deref(), &app_id)
        {
            unit_cleanup.cleanup().await;
            return Err(err);
        }
        unit_cleanup.disarm();
        cleanup.disarm();
        drop(port_reservation);
        Ok(record)
    }

    pub(super) async fn boot_unit(&self, spec: &StartUnitSpec) -> Result<(), StartupError> {
        if let Err(err) = supervisor::stop_unit(&spec.unit).await {
            return Err(StartupError::Failed(err.to_string()));
        }
        if let Err(err) = supervisor::start_unit(spec).await {
            return Err(StartupError::Failed(err.to_string()));
        }
        if let Err(err) = health::wait_until_healthy(
            &spec.unit,
            &spec.runtime_dir,
            spec.port,
            &spec.log_file,
            self.startup_timeout,
        )
        .await
        {
            if let Err(stop_err) = supervisor::stop_unit(&spec.unit).await {
                let reason = err.into_message();
                return Err(StartupError::Failed(format!(
                    "{reason} (stop unit failed: {stop_err})"
                )));
            }
            return Err(err);
        }
        Ok(())
    }

    pub(super) async fn boot_staging_unit(
        &self,
        spec: &StartSpec,
        apps_root: &Path,
    ) -> Result<(), String> {
        let stage_id = format!(".stage-{}", Uuid::new_v4());
        let raw_stage_dir = apps_root.join(&stage_id);
        let _dir_cleanup = DirCleanup::armed(raw_stage_dir.clone());
        create_private_dir(&raw_stage_dir).map_err(|e| e.to_string())?;
        let raw_scratch = raw_stage_dir.join(SCRATCH_DIR_NAME);
        create_private_dir(&raw_scratch).map_err(|e| e.to_string())?;
        let stage_dir = fs::canonicalize(&raw_stage_dir).map_err(|e| e.to_string())?;
        let scratch_dir = fs::canonicalize(&raw_scratch).map_err(|e| e.to_string())?;
        let log_file = stage_dir.join(LOG_FILE_NAME);

        let port_reservation = health::allocate_port(
            &used_ports(&self.database).map_err(|e| e.to_string())?,
            &spec.name,
        )
        .map_err(|e| e.to_string())?;
        let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
        let stage_unit = supervisor::staging_unit_name(&self.database, &spec.name, seq);
        let mut unit_cleanup = supervisor::UnitCleanup::armed_unit(stage_unit.clone());

        let unit_spec = StartUnitSpec {
            unit: stage_unit,
            app_name: spec.name.clone(),
            app_dir: spec.app_dir.clone(),
            scratch_dir,
            runtime_dir: stage_dir,
            log_file,
            command: spec.command.clone(),
            port: port_reservation.port(),
            base_path: format!("/apps/{}", spec.name),
        };
        let res = self.boot_unit(&unit_spec).await;
        unit_cleanup.cleanup().await;
        res.map_err(StartupError::into_message)
    }

    pub(super) async fn launch_existing(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        supervisor::preflight().await?;
        self.enforce_running_cap(Some(&rec.app.name)).await?;
        let app_dir = verify_existing_folder(&rec.app.source_folder)?;
        let apps_root = ensure_apps_root(&self.database)?;
        let (runtime_dir, scratch_dir) = ensure_runtime_dirs(&apps_root, &rec.app_id)?;
        let log_file = runtime_dir.join(LOG_FILE_NAME);
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &rec.app.name);
        let spec = StartUnitSpec {
            unit: supervisor::unit_name(&self.database, &rec.app.name),
            app_name: rec.app.name.clone(),
            app_dir,
            scratch_dir,
            runtime_dir,
            log_file,
            command: rec.app.command.clone(),
            port: rec.app.port,
            base_path: format!("/apps/{}", rec.app.name),
        };
        match self.boot_unit(&spec).await {
            Ok(()) => {}
            Err(StartupError::TimedOut(reason)) => {
                unit_cleanup.cleanup().await;
                let _ = set_state_stopped(&self.database, &rec.app.name, now_ms()?);
                return Err(AppError::StartupFailed { reason });
            }
            Err(StartupError::Failed(reason)) => {
                unit_cleanup.cleanup().await;
                let _ = set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?);
                return Err(AppError::StartupFailed { reason });
            }
        }
        set_state_running(&self.database, &rec.app.name, now_ms()?)?;
        unit_cleanup.disarm();
        read_record(&self.database, &rec.app.name)?
            .map(|r| r.app)
            .ok_or(AppError::NotFound)
    }

    pub(super) async fn wake_existing(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        match self.launch_existing(rec).await {
            Ok(app) => Ok(app),
            Err(AppError::StartupFailed { reason }) => {
                let latest = read_record(&self.database, &rec.app.name)?;
                if latest
                    .as_ref()
                    .is_some_and(|r| r.app.state == AppState::Stopped)
                {
                    Err(AppError::StartupFailed { reason })
                } else {
                    Err(AppError::Failed {
                        name: rec.app.name.clone(),
                        reason,
                        logs: supervisor::read_log_tail(&log_file_path(
                            &self.database,
                            &rec.app_id,
                        )),
                    })
                }
            }
            Err(other) => Err(other),
        }
    }
}

pub(super) fn verify_existing_folder(folder: &str) -> Result<PathBuf, AppError> {
    let app_dir = PathBuf::from(folder);
    let meta = fs::symlink_metadata(&app_dir).map_err(|err| {
        AppError::InvalidFolder(format!("cannot open `{}`: {err}", app_dir.display()))
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(AppError::InvalidFolder(format!(
            "`{}` must be a real folder",
            app_dir.display()
        )));
    }
    let canonical = fs::canonicalize(&app_dir).map_err(|err| {
        AppError::InvalidFolder(format!("cannot resolve `{}`: {err}", app_dir.display()))
    })?;
    if canonical != app_dir {
        return Err(AppError::InvalidFolder(format!(
            "`{}` must not traverse a symbolic link",
            app_dir.display()
        )));
    }
    Ok(app_dir)
}

fn ensure_real_dir(dir: &Path) -> Result<PathBuf, AppError> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(AppError::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{}` must be a real directory", dir.display()),
                )));
            }
            let canonical = fs::canonicalize(dir)?;
            let parent = dir.parent().unwrap_or_else(|| Path::new("."));
            let expected = fs::canonicalize(parent)?.join(dir.file_name().unwrap_or_default());
            if canonical != expected {
                return Err(AppError::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{}` must not traverse a symbolic link", dir.display()),
                )));
            }
            Ok(canonical)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            create_private_dir(dir)?;
            Ok(fs::canonicalize(dir)?)
        }
        Err(err) => Err(err.into()),
    }
}

pub(super) fn ensure_apps_root(database: &Path) -> Result<PathBuf, AppError> {
    let root = apps_dir_for_database(database);
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)?;
    }
    ensure_real_dir(&root)
}

pub(super) fn ensure_runtime_dirs(
    apps_root: &Path,
    app_id: &str,
) -> Result<(PathBuf, PathBuf), AppError> {
    let base = apps_root.join(app_id);
    let runtime_dir = ensure_real_dir(&base)?;
    let scratch = base.join(SCRATCH_DIR_NAME);
    let scratch_dir = ensure_real_dir(&scratch)?;
    Ok((runtime_dir, scratch_dir))
}

pub(super) fn apps_dir_for_database(database: &Path) -> PathBuf {
    match database.parent() {
        Some(state_dir) if state_dir.file_name().is_some_and(|name| name == "state") => state_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(APPS_DIRECTORY_PATH),
        Some(dir) => dir.join("apps"),
        None => PathBuf::from(APPS_DIRECTORY_PATH),
    }
}

pub(super) fn runtime_dir_for(database: &Path, app_id: &str) -> PathBuf {
    apps_dir_for_database(database).join(app_id)
}

pub(super) fn log_file_path(database: &Path, app_id: &str) -> PathBuf {
    runtime_dir_for(database, app_id).join(LOG_FILE_NAME)
}

pub(super) fn format_reason_with_logs(detail: &str, log_file: &Path) -> String {
    let tail = supervisor::read_log_tail(log_file);
    if tail.is_empty() {
        detail.to_owned()
    } else {
        format!("{detail}\n\nRecent logs:\n{tail}")
    }
}

pub(super) fn now_ms() -> Result<i64, AppError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis())
        .map_err(|_| AppError::Io(io::Error::other("system clock overflows i64 milliseconds")))
}
