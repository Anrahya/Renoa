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

/// Converges every plugin's trigger automations as of `now_ms`: the one it
/// wants is created, or updated to its wanted spec, and every other one it
/// owns is disabled. A schedule or switch that changes is due again from
/// `now_ms`, as an edit by its agent would be.
fn reconcile(tx: &Transaction<'_>, now_ms: i64) -> Result<(), AutomationError> {
    let owned = triggers::owned(tx).map_err(|error| plugin_error(&error))?;
    for plugin in HostPluginId::ALL {
        let wanted = triggers::wanted(plugin, tx).map_err(|error| plugin_error(&error))?;
        let target = wanted
            .as_ref()
            .map(|spec| triggers::automation_id(plugin, spec.agent_id));
        for (id, _) in owned
            .iter()
            .filter(|(id, owner)| *owner == plugin && Some(*id) != target)
        {
            let old = store::stored(tx, *id)?.ok_or(AutomationError::NotFound)?;
            if old.spec.enabled {
                let mut spec = old.spec.clone();
                spec.enabled = false;
                store::save(
                    tx,
                    &store::updated_record(&old, old.revision, spec, now_ms)?,
                    false,
                )?;
            }
        }
        let (Some(spec), Some(id)) = (wanted, target) else {
            continue;
        };
        match store::stored(tx, id)? {
            Some(old) if old.spec == spec => {}
            Some(old) => store::save(
                tx,
                &store::updated_record(&old, old.revision, spec, now_ms)?,
                false,
            )?,
            None => {
                spec.validate(now_ms)?;
                let record = AutomationRecord {
                    id,
                    revision: 1,
                    next_due_ms: spec.schedule.first_due(now_ms, spec.enabled)?,
                    spec,
                };
                store::save(tx, &record, true)?;
                triggers::own(tx, id, plugin)?;
            }
        }
    }
    Ok(())
}

fn plugin_error(error: &crate::plugins::PluginError) -> AutomationError {
    AutomationError::Invalid(format!("a plugin trigger could not be read: {error}"))
}
