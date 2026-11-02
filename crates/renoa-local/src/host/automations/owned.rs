//! Keeps each plugin's trigger automation as its plugin wants it.
//!
//! The scheduler converges them inside the transaction that admits the next
//! due run, so a change to what decides a trigger, such as the main agent,
//! the plugin's switch or the notebook zone, takes effect before the next
//! admission without a hook in each place that changes it.

use rusqlite::Transaction;

use super::{AutomationError, AutomationRecord, store};
use crate::plugins::host::{HostPluginId, triggers};

/// Converges the triggers as [`reconcile`] does, inside a savepoint: a
/// trigger that cannot be converged keeps every stored trigger as it was and
/// is logged, and never stops an agent's due run from being admitted.
pub(super) fn converge(tx: &Transaction<'_>, now_ms: i64) -> Result<(), AutomationError> {
    tx.execute_batch("SAVEPOINT plugin_triggers")?;
    match reconcile(tx, now_ms) {
        Ok(()) => tx.execute_batch("RELEASE plugin_triggers")?,
        Err(error) => {
            tx.execute_batch("ROLLBACK TO plugin_triggers; RELEASE plugin_triggers")?;
            renoa_telemetry::event(
                "renoa.host",
                "warn",
                "plugin_triggers_unconverged",
                &serde_json::json!({ "error": error.to_string() }),
            );
        }
    }
    Ok(())
}

/// Converges every plugin's trigger automations as of `now_ms`: each one it
/// wants is created, or updated to its wanted spec, and every other one it
/// owns is disabled. A schedule or switch that changes is due again from
/// `now_ms`, as an edit by its agent would be; a one-time schedule at or
/// before `now_ms` is due at once.
fn reconcile(tx: &Transaction<'_>, now_ms: i64) -> Result<(), AutomationError> {
    let owned = triggers::owned(tx).map_err(|error| plugin_error(&error))?;
    for plugin in HostPluginId::ALL {
        let wanted = triggers::wanted(plugin, tx, now_ms).map_err(|error| plugin_error(&error))?;
        for (id, _) in owned.iter().filter(|(id, owner)| {
            *owner == plugin && !wanted.iter().any(|(target, _)| target == id)
        }) {
            let old = store::stored(tx, *id)?.ok_or(AutomationError::NotFound)?;
            if old.spec.enabled {
                let mut spec = old.spec.clone();
                spec.enabled = false;
                store::save(tx, &store::updated_trigger(&old, spec, now_ms)?, false)?;
            }
        }
        for (id, spec) in wanted {
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
                    triggers::own(tx, id, plugin)?;
                }
            }
        }
    }
    Ok(())
}

fn plugin_error(error: &crate::plugins::PluginError) -> AutomationError {
    AutomationError::Invalid(format!("a plugin trigger could not be read: {error}"))
}
