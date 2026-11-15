//! Idle sweeping, capacity eviction, startup reconciliation, and systemd sync.

use std::{collections::HashSet, fs};

use super::{
    AppError, AppState, Apps, LiveApp, health,
    launch::{apps_dir_for_database, format_reason_with_logs, log_file_path, now_ms},
    model::MAX_RUNNING_APPS,
    store::{AppRecord, list_all_records, read_record, set_state_failed, set_state_stopped},
    supervisor::{self, UnitStatus},
    unit_names,
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

    /// Removes every entry under the apps directory that no `host_apps` row
    /// names, including staging directories a crash left behind. Call it
    /// before anything can start an app: a start creates its directory before
    /// committing its row, so a concurrent sweep would delete it.
    ///
    /// # Errors
    /// Returns [`AppError`] when the catalog cannot be read.
    pub fn sweep_orphan_dirs(&self) -> Result<usize, AppError> {
        let known: HashSet<String> = list_all_records(&self.database)?
            .into_iter()
            .map(|rec| rec.app_id)
            .collect();
        let Ok(entries) = fs::read_dir(apps_dir_for_database(&self.database)) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(|id| known.contains(id)) {
                continue;
            }
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Reconciles `host_apps` with `systemd --user` when `renoa-node` starts.
    /// It runs while the gateway serves, so each app is re-read under its
    /// lock instead of trusting a snapshot taken before a concurrent start.
    ///
    /// # Errors
    /// Returns [`AppError`] on catalog read or transient `systemd --user` unavailability.
    pub async fn reconcile_on_startup(&self) -> Result<(), AppError> {
        let mut first_supervisor_err: Option<AppError> = None;
        let names: Vec<String> = list_all_records(&self.database)?
            .into_iter()
            .map(|rec| rec.app.name)
            .collect();

        for name in names {
            let mutex = self.lock_for(&name);
            let _guard = mutex.lock().await;
            let Some(rec) = read_record(&self.database, &name)? else {
                continue;
            };
            let unit = unit_names::unit_name(&self.database, &name);
            let status = supervisor::unit_status(&unit).await;
            match (rec.app.state, status) {
                (_, Err(err)) => {
                    first_supervisor_err.get_or_insert(AppError::SupervisorUnavailable(format!(
                        "unit status check failed during startup: {err}"
                    )));
                }
                (AppState::Running, Ok(UnitStatus::Failed { detail })) => {
                    let _ = set_state_failed(&self.database, &name, &detail, now_ms()?);
                }
                (AppState::Running, Ok(UnitStatus::Inactive)) => {
                    let _ = self.launch_existing(&rec).await;
                }
                (AppState::Stopped, Ok(UnitStatus::Active { .. } | UnitStatus::Activating)) => {
                    let _ = supervisor::stop_unit(&unit).await;
                }
                _ => {}
            }
        }

        match supervisor::list_home_units(&self.database).await {
            Ok(units) => {
                let prefix = unit_names::unit_prefix_for(&self.database);
                for unit in units {
                    let Some(owner) = unit
                        .strip_prefix(&prefix)
                        .and_then(|rest| rest.strip_suffix(".service"))
                        .map(unit_names::unit_owner)
                    else {
                        continue;
                    };
                    let mutex = self.lock_for(owner.name);
                    let _guard = mutex.lock().await;
                    if owner.staging || read_record(&self.database, owner.name)?.is_none() {
                        let _ = supervisor::stop_unit(&unit).await;
                    }
                }
            }
            Err(err) => {
                first_supervisor_err.get_or_insert(AppError::SupervisorUnavailable(format!(
                    "list units failed during startup: {err}"
                )));
            }
        }

        first_supervisor_err.map_or(Ok(()), Err)
    }

    pub(super) async fn sync_live_state(&self, rec: &AppRecord) -> Result<LiveApp, AppError> {
        if rec.app.state != AppState::Running {
            return Ok(rec.app.clone());
        }
        let unit = unit_names::unit_name(&self.database, &rec.app.name);
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
        let unit = unit_names::unit_name(&self.database, &rec.app.name);
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

        match health::check_unit_listener(main_pid, rec.app.port) {
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
