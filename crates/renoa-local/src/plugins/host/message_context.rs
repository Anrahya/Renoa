//! The `message_context` point: the entries a newly admitted prompt carries.
//!
//! The receiving surface's own entry comes first, then one per enabled
//! plugin. They are computed once, before admission, and frozen into the
//! command, so a retry or recovery reuses them and enabling, disabling, or
//! reconfiguring a plugin later cannot change them. An entry that fails is
//! skipped and reported; it never blocks the message.

use std::path::Path;

use renoa_agent_loop::{ContextContribution, TurnContext};
use renoa_kernel::AgentId;

use super::{HostPluginId, settings, state, time::TimeSettings};
use crate::{Speaker, host::catalog, plugins::PluginError};

/// A contributor left out of one message, and why: `surface`, a plugin id,
/// or `*` when no plugin could be asked.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Skipped {
    pub(crate) source: &'static str,
    pub(crate) reason: String,
}

/// The context for a prompt from `speaker` admitted at `observed_at_ms`, after
/// a previous prompt admitted at `previous_ms`, whose surface described where
/// it was written as `surface`. Plugins contribute only when the speaker
/// admits plugin context.
pub(crate) fn admit(
    database: &Path,
    agent: AgentId,
    speaker: Speaker,
    surface: Option<&str>,
    observed_at_ms: i64,
    previous_ms: Option<i64>,
) -> (TurnContext, Vec<Skipped>) {
    let mut context = TurnContext::default();
    let mut skipped = Vec::new();
    if let Some(text) = surface {
        let added = ContextContribution::surface(text)
            .map_err(|error| PluginError::Invalid(error.to_string()))
            .map(Some);
        add(&mut context, &mut skipped, "surface", added);
    }
    if !speaker.admits_plugin_context() {
        return (context, skipped);
    }
    let db = match catalog::open_verified(database) {
        Ok(db) => db,
        Err(error) => {
            skipped.push(Skipped {
                source: "*",
                reason: error.to_string(),
            });
            return (context, skipped);
        }
    };
    for plugin in HostPluginId::ALL {
        let added = contribution(&db, agent, plugin, observed_at_ms, previous_ms);
        add(&mut context, &mut skipped, plugin.id(), added);
    }
    (context, skipped)
}

/// Appends one contributor's entry, or records why it is left out. Only that
/// contributor is left out; the entries before it stay.
fn add(
    context: &mut TurnContext,
    skipped: &mut Vec<Skipped>,
    source: &'static str,
    entry: Result<Option<ContextContribution>, PluginError>,
) {
    let grown = entry.and_then(|entry| {
        entry.map_or(Ok(None), |entry| {
            let mut entries = context.entries().to_vec();
            entries.push(entry);
            TurnContext::new(entries)
                .map(Some)
                .map_err(|error| PluginError::Invalid(error.to_string()))
        })
    });
    match grown {
        Ok(Some(grown)) => *context = grown,
        Ok(None) => {}
        Err(error) => skipped.push(Skipped {
            source,
            reason: error.to_string(),
        }),
    }
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
