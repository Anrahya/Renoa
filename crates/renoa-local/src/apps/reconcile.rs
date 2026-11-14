//! Idle sweeping, startup reconciliation, boot/health coordination, and runtime
//! directory helpers for live apps.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    AppError, AppState, Apps, LiveApp, health,
    model::{APPS_RELATIVE_SUBDIR, LOG_FILE_NAME, SCRATCH_DIR_NAME},
    store::{AppRecord, list_all_records, read_record, set_state_failed, set_state_running},
    supervisor::{self, StartUnitSpec, UnitStatus},
};
use crate::sites::{create_private_dir, ensure_state_subdir};

impl Apps {
    /// Stops any running app whose `last_active_ms` is older than `idle_timeout`.
    /// Continues across individual app errors and returns an error only if no
    /// apps were stopped and a stop failed.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog read failure or if every attempted stop fails.
    pub async fn stop_idle(&self) -> Result<Vec<String>, AppError> {
        let idle_ms = i64::try_from(self.idle_timeout.as_millis()).unwrap_or(i64::MAX);
        let cutoff = now_ms()? - idle_ms;
        let mut stopped = Vec::new();
        let mut first_err: Option<AppError> = None;

        for rec in list_all_records(&self.database)? {
            if rec.app.state != AppState::Running || rec.app.last_active_ms > cutoff {
                continue;
            }
            let mutex = self.lock_for(&rec.app.name);
            let _guard = mutex.lock().await;
            let fresh = match read_record(&self.database, &rec.app.name) {
                Ok(Some(fresh)) => fresh,
                Ok(None) => continue,
                Err(err) => {
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                    continue;
                }
            };
            if fresh.app.state == AppState::Running
                && fresh.app.last_active_ms <= now_ms()? - idle_ms
            {
                match self.stop_locked(&fresh).await {
                    Ok(_) => stopped.push(fresh.app.name),
                    Err(err) => {
                        if first_err.is_none() {
                            first_err = Some(err);
                        }
                    }
                }
            }
        }
        if stopped.is_empty()
            && let Some(err) = first_err
        {
            return Err(err);
        }
        Ok(stopped)
    }

    /// Reconciles `host_apps` with `systemd --user` when `renoa-node` starts.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog read failure.
    pub async fn reconcile_on_startup(&self) -> Result<(), AppError> {
        let records = list_all_records(&self.database)?;
        let mut known_names = HashSet::new();
        let mut known_ids = HashSet::new();

        for rec in &records {
            known_names.insert(rec.app.name.clone());
            known_ids.insert(rec.app_id.clone());

            let mutex = self.lock_for(&rec.app.name);
            let _guard = mutex.lock().await;
            let unit = supervisor::unit_name(&self.database, &rec.app.name);
            match rec.app.state {
                AppState::Running => match supervisor::unit_status(&unit).await {
                    Ok(UnitStatus::Failed { detail }) => {
                        let _ = set_state_failed(&self.database, &rec.app.name, &detail, now_ms()?);
                    }
                    Ok(UnitStatus::Inactive) => {
                        let _ = self.launch_existing(rec).await;
                    }
                    Ok(UnitStatus::Active) | Err(_) => {}
                },
                AppState::Stopped => {
                    if matches!(supervisor::unit_status(&unit).await, Ok(UnitStatus::Active)) {
                        let _ = supervisor::stop_unit(&unit).await;
                    }
                }
                AppState::Failed => {}
            }
        }

        let prefix = supervisor::unit_prefix_for(&self.database);
        if let Ok(units) = supervisor::list_home_units(&self.database).await {
            for unit in units {
                if let Some(app_name) = unit
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.strip_suffix(".service"))
                    && !known_names.contains(app_name)
                {
                    let _ = supervisor::stop_unit(&unit).await;
                }
            }
        }

        if let Ok(entries) = fs::read_dir(apps_dir_for_database(&self.database)) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if let Some(dir_name) = name.to_str()
                    && !known_ids.contains(dir_name)
                {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }

        Ok(())
    }

    pub(super) async fn boot_unit(
        &self,
        name: &str,
        app_dir: PathBuf,
        scratch_dir: PathBuf,
        log_file: PathBuf,
        command: String,
        port: u16,
    ) -> Result<(), String> {
        let unit = supervisor::unit_name(&self.database, name);
        if let Err(err) = supervisor::stop_unit(&unit).await {
            return Err(err.to_string());
        }
        let spec = StartUnitSpec {
            unit: unit.clone(),
            app_name: name.to_owned(),
            app_dir,
            scratch_dir,
            log_file: log_file.clone(),
            command,
            port,
            base_path: format!("/apps/{name}"),
        };
        if let Err(err) = supervisor::start_unit(&spec).await {
            return Err(err.to_string());
        }
        if let Err(reason) =
            health::wait_until_healthy(&unit, port, &log_file, self.startup_timeout).await
        {
            if let Err(stop_err) = supervisor::stop_unit(&unit).await {
                return Err(format!("{reason} (stop unit failed: {stop_err})"));
            }
            return Err(reason);
        }
        Ok(())
    }

    pub(super) async fn launch_existing(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        supervisor::preflight().await?;
        let app_dir = verify_existing_folder(&rec.app.source_folder)?;
        let apps_root = ensure_state_subdir(&self.database, APPS_RELATIVE_SUBDIR)?;
        let scratch = ensure_runtime_dirs(&apps_root, &rec.app_id)?;
        let log_file = apps_root.join(&rec.app_id).join(LOG_FILE_NAME);
        let mut unit_cleanup = supervisor::UnitCleanup::armed(&self.database, &rec.app.name);
        if let Err(reason) = self
            .boot_unit(
                &rec.app.name,
                app_dir,
                scratch,
                log_file,
                rec.app.command.clone(),
                rec.app.port,
            )
            .await
        {
            let _ = set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?);
            return Err(AppError::StartupFailed { reason });
        }
        set_state_running(&self.database, &rec.app.name, now_ms()?)?;
        unit_cleanup.disarm();
        read_record(&self.database, &rec.app.name)?
            .map(|r| r.app)
            .ok_or(AppError::NotFound)
    }

    pub(super) async fn sync_live_state(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        if rec.app.state != AppState::Running {
            return Ok(rec.app.clone());
        }
        let unit = supervisor::unit_name(&self.database, &rec.app.name);
        match supervisor::unit_status(&unit).await {
            Ok(UnitStatus::Failed { detail }) => {
                let reason =
                    format_reason_with_logs(&detail, &log_file_path(&self.database, &rec.app_id));
                set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?)?;
                Ok(LiveApp {
                    state: AppState::Failed,
                    failure_reason: Some(reason),
                    ..rec.app.clone()
                })
            }
            Ok(UnitStatus::Inactive) => {
                let reason = format_reason_with_logs(
                    "app exited unexpectedly",
                    &log_file_path(&self.database, &rec.app_id),
                );
                set_state_failed(&self.database, &rec.app.name, &reason, now_ms()?)?;
                Ok(LiveApp {
                    state: AppState::Failed,
                    failure_reason: Some(reason),
                    ..rec.app.clone()
                })
            }
            Ok(UnitStatus::Active) | Err(_) => Ok(rec.app.clone()),
        }
    }

    pub(super) async fn verify_live_loopback_http(&self, rec: &AppRecord) -> Result<bool, String> {
        match health::check_loopback_only_listener(rec.app.port) {
            Ok(true) => Ok(health::probe_http(rec.app.port).await),
            Ok(false) => Ok(false),
            Err(violation) => {
                let reason = violation.message();
                let unit = supervisor::unit_name(&self.database, &rec.app.name);
                let _ = supervisor::stop_unit(&unit).await;
                if let Ok(now) = now_ms() {
                    let _ = set_state_failed(&self.database, &rec.app.name, &reason, now);
                }
                Err(reason)
            }
        }
    }

    pub(super) fn failed_error(&self, rec: &AppRecord) -> AppError {
        AppError::Failed {
            name: rec.app.name.clone(),
            reason: rec
                .app
                .failure_reason
                .clone()
                .unwrap_or_else(|| "app failed".to_owned()),
            logs: supervisor::read_log_tail(&log_file_path(&self.database, &rec.app_id)),
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

fn ensure_real_dir(dir: &Path) -> Result<(), AppError> {
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
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            create_private_dir(dir).map_err(Into::into)
        }
        Err(err) => Err(err.into()),
    }
}

pub(super) fn ensure_runtime_dirs(apps_root: &Path, app_id: &str) -> Result<PathBuf, AppError> {
    let base = apps_root.join(app_id);
    ensure_real_dir(&base)?;
    let scratch = base.join(SCRATCH_DIR_NAME);
    ensure_real_dir(&scratch)?;
    Ok(scratch)
}

pub(super) fn apps_dir_for_database(database: &Path) -> PathBuf {
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(APPS_RELATIVE_SUBDIR)
}

pub(super) fn log_file_path(database: &Path, app_id: &str) -> PathBuf {
    apps_dir_for_database(database)
        .join(app_id)
        .join(LOG_FILE_NAME)
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
