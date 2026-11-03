//! Keeps each plugin's trigger automation as its plugin wants it.
//!
//! The scheduler converges them inside the transaction that admits the next
//! due run, so a change to what decides a trigger, such as the main agent,
//! the plugin's switch or the notebook zone, takes effect before the next
//! admission without a hook in each place that changes it.

use rusqlite::Transaction;
use uuid::Uuid;

use super::{AutomationError, AutomationRecord, retention, runs, store};
use crate::{
    AutomationSpec,
    plugins::host::{
        HostPluginId,
        triggers::{self, Prepared, Reminder},
    },
};

/// Converges the triggers as [`reconcile`] does, inside a savepoint: a
/// trigger that cannot be converged keeps every stored trigger as it was and
/// is logged, and never stops an agent's due run from being admitted.
/// Returns what retention removed, to report once committed.
pub(super) fn converge(
    tx: &Transaction<'_>,
    prepared: &Prepared,
    now_ms: i64,
) -> Result<Vec<retention::Removed>, AutomationError> {
    tx.execute_batch("SAVEPOINT plugin_triggers")?;
    match reconcile(tx, prepared, now_ms) {
        Ok(removed) => {
            tx.execute_batch("RELEASE plugin_triggers")?;
            Ok(removed)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO plugin_triggers; RELEASE plugin_triggers")?;
            renoa_telemetry::event(
                "renoa.host",
                "warn",
                "plugin_triggers_unconverged",
                &serde_json::json!({ "error": error.to_string() }),
            );
            Ok(Vec::new())
        }
    }
}

/// Converges every plugin's trigger automations as of `now_ms`: each one it
/// wants is created, or updated to its wanted spec, and every other one it
/// owns is disabled. A schedule or switch that changes is due again from
/// `now_ms`, as an edit by its agent would be; a one-time schedule at or
/// before `now_ms` is due at once. A reminder is retired once it is not
/// wanted (see [`retention::retire_trigger`]); every other trigger is kept,
/// disabled, so its runs and its task stay. One
/// wanted trigger whose spec cannot be stored or scheduled is left as it is
/// and logged as `automation_unschedulable`, as a due row that cannot be
/// admitted is, so it holds up no other.
fn reconcile(
    tx: &Transaction<'_>,
    prepared: &Prepared,
    now_ms: i64,
) -> Result<Vec<retention::Removed>, AutomationError> {
    let owned = triggers::owned(tx).map_err(|error| plugin_error(&error))?;
    let mut removed = Vec::new();
    for plugin in HostPluginId::ALL {
        let wanted =
            triggers::wanted(plugin, tx, prepared, now_ms).map_err(|error| plugin_error(&error))?;
        for (id, _) in owned
            .iter()
            .filter(|(id, owner)| *owner == plugin && !wanted.iter().any(|target| target.id == *id))
        {
            let old = store::stored(tx, *id)?.ok_or(AutomationError::NotFound)?;
            if old.spec.enabled {
                let mut spec = old.spec.clone();
                spec.enabled = false;
                store::save(tx, &store::updated_trigger(&old, spec, now_ms)?, false)?;
            }
            if triggers::delivers(tx, *id)? {
                removed.extend(retention::retire_trigger(tx, *id)?);
            }
        }
        for triggers::Wanted { id, spec, reminder } in wanted {
            tx.execute_batch("SAVEPOINT plugin_trigger")?;
            match keep(tx, plugin, id, spec, reminder.as_ref(), now_ms) {
                Ok(()) => tx.execute_batch("RELEASE plugin_trigger")?,
                Err(error) if runs::of_its_row(&error) => {
                    tx.execute_batch("ROLLBACK TO plugin_trigger; RELEASE plugin_trigger")?;
                    runs::report_unschedulable(
                        "automation_id",
                        &id.to_string(),
                        &error.to_string(),
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(removed)
}

/// Creates `plugin`'s trigger `id` as `spec`, or updates it to `spec`.
fn keep(
    tx: &Transaction<'_>,
    plugin: HostPluginId,
    id: Uuid,
    spec: AutomationSpec,
    reminder: Option<&Reminder>,
    now_ms: i64,
) -> Result<(), AutomationError> {
    match store::stored(tx, id)? {
        Some(old) if old.spec == spec => {}
        Some(old) => {
            store::save(tx, &store::updated_trigger(&old, spec, now_ms)?, false)?;
        }
        None => {
            spec.validate(now_ms)?;
            let record = AutomationRecord {
                id,
                revision: 1,
                next_due_ms: spec.schedule.next_after(now_ms)?,
                spec,
            };
            store::save(tx, &record, true)?;
            triggers::own(tx, id, plugin, reminder)?;
        }
    }
    Ok(())
}

fn plugin_error(error: &crate::plugins::PluginError) -> AutomationError {
    AutomationError::Invalid(format!("a plugin trigger could not be read: {error}"))
}
