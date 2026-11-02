//! When the nightly run is due and for whom: the trigger `renoa.memory`
//! contributes, which the scheduler keeps converged.

use renoa_kernel::AgentId;
use rusqlite::Connection;

use super::super::notebook_zone;
use crate::{
    AutomationSchedule, AutomationSpec,
    host::definition::main_agent,
    plugins::{
        PluginError,
        host::{HostPluginId, state},
    },
};

const NAME: &str = "renoa.memory nightly";
const TASK: &str = "Compact every notebook on this Host, then dream: write each person's MIND.md from what was noted since the last dream.";
/// 04:00 in the notebook zone, when the person is least likely to be talking.
const EXPRESSION: &str = "0 4 * * *";

/// The agent whose memory session drives every notebook: the Host's main
/// agent, while `renoa.memory` is on for it. One driver per notebook, so the
/// `mnem` command's compactor must never run on one too.
pub(crate) fn driver(db: &Connection) -> Result<Option<AgentId>, PluginError> {
    let Some(agent) = main_agent(db)? else {
        return Ok(None);
    };
    Ok(state::enabled_in(db, agent, HostPluginId::Memory)?.then_some(agent))
}

/// The nightly trigger, for the [`driver`] and in the notebook zone. A zone
/// without an IANA name, which a cron schedule needs, keeps 04:00 in UTC.
pub(crate) fn wanted(db: &Connection) -> Result<Option<AutomationSpec>, PluginError> {
    let Some(agent) = driver(db)? else {
        return Ok(None);
    };
    let zone = notebook_zone(db)?;
    Ok(Some(AutomationSpec {
        agent_id: agent,
        name: NAME.to_owned(),
        prompt: TASK.to_owned(),
        schedule: AutomationSchedule::Cron {
            expression: EXPRESSION.to_owned(),
            timezone: zone.iana_name().unwrap_or("UTC").to_owned(),
        },
        enabled: true,
    }))
}
