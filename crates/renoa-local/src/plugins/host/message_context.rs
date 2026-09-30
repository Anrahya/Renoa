//! The `message_context` point: the entries a newly admitted prompt carries.
//!
//! They are computed once, before admission, and frozen into the command, so a
//! retry or recovery reuses them and enabling, disabling, or reconfiguring a
//! plugin later cannot change them. A contributor that fails is skipped and
//! reported; it never blocks the message.

use std::path::Path;

use renoa_agent_loop::{ContextContribution, TurnContext};
use renoa_kernel::AgentId;

use super::{HostPluginId, settings, state, time::TimeSettings};
use crate::{host::catalog, plugins::PluginError};

/// A contributor left out of one message, and why.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Skipped {
    pub(crate) plugin_id: &'static str,
    pub(crate) reason: String,
}

/// The context for a prompt admitted at `observed_at_ms`, after a previous
/// prompt admitted at `previous_ms`.
pub(crate) fn admit(
    database: &Path,
    agent: AgentId,
    observed_at_ms: i64,
    previous_ms: Option<i64>,
) -> (TurnContext, Vec<Skipped>) {
    let db = match catalog::open_verified(database) {
        Ok(db) => db,
        Err(error) => {
            return (
                TurnContext::default(),
                vec![Skipped {
                    plugin_id: "*",
                    reason: error.to_string(),
                }],
            );
        }
    };
    let mut context = TurnContext::default();
    let mut skipped = Vec::new();
    for plugin in HostPluginId::ALL {
        let added =
            contribution(&db, agent, plugin, observed_at_ms, previous_ms).and_then(|entry| {
                entry.map_or(Ok(None), |entry| {
                    let mut entries = context.entries().to_vec();
                    entries.push(entry);
                    TurnContext::new(entries)
                        .map(Some)
                        .map_err(|error| PluginError::Invalid(error.to_string()))
                })
            });
        match added {
            Ok(Some(grown)) => context = grown,
            Ok(None) => {}
            // Only this contributor is left out; the entries before it stay.
            Err(error) => skipped.push(Skipped {
                plugin_id: plugin.id(),
                reason: error.to_string(),
            }),
        }
    }
    (context, skipped)
}

/// One plugin's entry, or none when it contributes nothing to this message.
fn contribution(
    db: &rusqlite::Connection,
    agent: AgentId,
    plugin: HostPluginId,
    observed_at_ms: i64,
    previous_ms: Option<i64>,
) -> Result<Option<ContextContribution>, PluginError> {
    match plugin {
        HostPluginId::Time => {
            if !state::enabled_in(db, agent, plugin)? {
                return Ok(None);
            }
            TimeSettings::validate(settings::read_in(db, agent, plugin)?)?
                .contribution(observed_at_ms, previous_ms)
                .map(Some)
        }
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git => Ok(None),
    }
}
