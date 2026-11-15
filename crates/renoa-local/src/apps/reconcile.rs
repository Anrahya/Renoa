//! Idle sweeping, capacity eviction, startup reconciliation, and systemd sync.

use std::{collections::HashSet, fs};

use super::{
    AppError, AppState, Apps, LiveApp, health,
    launch::{
        apps_dir_for_database, format_reason_with_logs, log_file_path, now_ms, runtime_dir_for,
    },
    model::MAX_RUNNING_APPS,
    store::{AppRecord, list_all_records, read_record, set_state_failed, set_state_stopped},
    supervisor::{self, UnitStatus},
};

impl Apps {
    /// Stops any running app whose `last_active_ms` is older than `idle_timeout`
    /// and that has no open proxy/WebSocket streams.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog read failure or if every attempted stop fails.
    pub async fn stop_idle(&self) -> Result<Vec<String>, AppError> {
        let idle_ms = i64::try_from(self.idle_timeout.as_millis()).unwrap_or(i64::MAX);
        let cutoff = now_ms()? - idle_ms;
        let mut stopped = Vec::new();
        let mut first_err: Option<AppError> = None;

        for rec in list_all_records(&self.database)? {
            if rec.app.state != AppState::Running
                || rec.app.last_active_ms > cutoff
                || self.active_stream_count(&rec.app.name) > 0
            {
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
                && self.active_stream_count(&fresh.app.name) == 0
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

    pub(super) async fn enforce_running_cap(
        &self,
        excluding: Option<&str>,
    ) -> Result<(), AppError> {
        let mut candidates: Vec<AppRecord> = list_all_records(&self.database)?
            .into_iter()
            .filter(|rec| {
                rec.app.state == AppState::Running && excluding != Some(rec.app.name.as_str())
            })
            .collect();
        if candidates.len() < MAX_RUNNING_APPS {
            return Ok(());
        }
        candidates.sort_by_key(|rec| rec.app.last_active_ms);
        let mut remaining = candidates.len() + 1 - MAX_RUNNING_APPS;

        for cand in candidates {
            if remaining == 0 {
                break;
            }
            if self.active_stream_count(&cand.app.name) > 0 {
                continue;
            }
            let mutex = self.lock_for(&cand.app.name);
            let Ok(_guard) = mutex.try_lock() else {
                continue;
            };
            let Some(fresh) = read_record(&self.database, &cand.app.name)? else {
                remaining = remaining.saturating_sub(1);
                continue;
            };
            if fresh.app.state != AppState::Running {
                remaining = remaining.saturating_sub(1);
                continue;
            }
            if self.active_stream_count(&fresh.app.name) > 0 {
                continue;
            }
            if self.stop_locked(&fresh).await.is_ok() {
                remaining = remaining.saturating_sub(1);
            }
        }

        if remaining > 0 {
            return Err(AppError::StartupFailed {
                reason: format!(
                    "all {MAX_RUNNING_APPS} running live app slots are busy with active streams"
                ),
            });
        }
        Ok(())
    }

    /// Reconciles `host_apps` with `systemd --user` when `renoa-node` starts.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog read or transient `systemd --user` unavailability.
    pub async fn reconcile_on_startup(&self) -> Result<(), AppError> {
        let records = list_all_records(&self.database)?;
        let mut known_names = HashSet::new();
        let mut known_ids = HashSet::new();
        let mut first_supervisor_err: Option<AppError> = None;

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
                    Ok(UnitStatus::Active { .. } | UnitStatus::Activating) => {}
                    Err(err) => {
                        if first_supervisor_err.is_none() {
                            first_supervisor_err = Some(AppError::SupervisorUnavailable(format!(
                                "unit status check failed during startup: {err}"
                            )));
                        }
                    }
                },
                AppState::Stopped => match supervisor::unit_status(&unit).await {
                    Ok(UnitStatus::Active { .. } | UnitStatus::Activating) => {
                        let _ = supervisor::stop_unit(&unit).await;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        if first_supervisor_err.is_none() {
                            first_supervisor_err = Some(AppError::SupervisorUnavailable(format!(
                                "unit status check failed during startup: {err}"
                            )));
                        }
                    }
                },
                AppState::Failed => {}
            }
        }

        let prefix = supervisor::unit_prefix_for(&self.database);
        match supervisor::list_home_units(&self.database).await {
            Ok(units) => {
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
            Err(err) => {
                if first_supervisor_err.is_none() {
                    first_supervisor_err = Some(AppError::SupervisorUnavailable(format!(
                        "list units failed during startup: {err}"
                    )));
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

        if let Some(err) = first_supervisor_err {
            return Err(err);
        }
        Ok(())
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
                set_state_stopped(&self.database, &rec.app.name, now_ms()?)?;
                Ok(LiveApp {
                    state: AppState::Stopped,
                    failure_reason: None,
                    ..rec.app.clone()
                })
            }
            Ok(UnitStatus::Active { .. } | UnitStatus::Activating) => Ok(rec.app.clone()),
            Err(err) => Err(AppError::SupervisorUnavailable(format!(
                "cannot query unit status for `{}`: {err}",
                rec.app.name
            ))),
        }
    }

    pub(super) async fn verify_live_loopback_http(&self, rec: &AppRecord) -> Result<bool, String> {
        let unit = supervisor::unit_name(&self.database, &rec.app.name);
        let main_pid = match supervisor::unit_status(&unit).await {
            Ok(UnitStatus::Active { main_pid }) => main_pid,
            Ok(UnitStatus::Activating | UnitStatus::Inactive) => return Ok(false),
            Ok(UnitStatus::Failed { detail }) => {
                let _ = supervisor::stop_unit(&unit).await;
                if let Ok(now) = now_ms() {
                    let _ = set_state_failed(&self.database, &rec.app.name, &detail, now);
                }
                return Err(detail);
            }
            Err(err) => return Err(format!("cannot inspect unit `{unit}`: {err}")),
        };

        let runtime_dir = runtime_dir_for(&self.database, &rec.app_id);
        match health::check_unit_listener(main_pid, &runtime_dir, rec.app.port) {
            Ok(true) => Ok(health::probe_http(rec.app.port).await),
            Ok(false) => Ok(false),
            Err(violation) => {
                let reason = violation.message();
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
