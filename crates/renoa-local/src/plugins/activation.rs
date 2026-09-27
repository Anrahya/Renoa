use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::{PluginError, PluginManager};
use crate::skills::SkillComponentReport;

/// One agent's selection. Identity begins with the first selected digest and
/// survives explicit replacement; display names never identify this record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginActivation {
    pub plugin_id: String,
    pub package_digest: String,
    pub enabled: bool,
    /// Session-loaded instructions stay pinned; these changes govern discovery
    /// and future tool resolution. A new session loads newer skill instructions.
    pub session_instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillComponentReport>,
}

#[derive(Serialize)]
pub(crate) enum ActivationChange {
    Activate {
        package_digest: String,
    },
    Replace {
        plugin_id: String,
        package_digest: String,
        expected_digest: String,
    },
    Enable {
        plugin_id: String,
    },
    Deactivate {
        plugin_id: String,
    },
}

pub(crate) mod schema;

impl PluginManager {
    pub(super) async fn preflight_revision(
        &self,
        agent: &renoa_kernel::AgentId,
        digest: &str,
    ) -> Result<(), PluginError> {
        let store = self.store.clone();
        let agent = agent.to_string();
        let digest = digest.to_owned();
        tokio::task::spawn_blocking(move || {
            super::store::validate_digest(&digest)?;
            require_revision_selection(&store.connection()?, &agent, &digest)?;
            Ok(())
        })
        .await?
    }

    pub(super) async fn preflight_account(
        &self,
        agent: &renoa_kernel::AgentId,
        account: &str,
    ) -> Result<(), PluginError> {
        let store = self.store.clone();
        let agent = agent.to_string();
        let account = account.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut connection = store.connection()?;
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            admit_connection_selection(&tx, &agent, &account)?;
            tx.rollback()?;
            Ok(())
        })
        .await?
    }

    pub(crate) async fn change_activation(
        &self,
        agent: &renoa_kernel::AgentId,
        change: ActivationChange,
        operation_id: &str,
    ) -> Result<PluginActivation, PluginError> {
        let store = self.store.clone();
        let skills = self.skills.clone();
        let agent = agent.to_string();
        let operation_id = operation_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let request = serde_json::to_string(&change)?;
            let mut connection = store.connection()?;
            // Check replay before loading content that may no longer be selected.
            if let Some(result) = receipt(&connection, &agent, &operation_id, &request)? {
                return Ok(result);
            }
            let (plugin_id, digest, expected, enabled) = match &change {
                ActivationChange::Activate { package_digest } => {
                    let owner = revision_owner(&connection,&agent,package_digest)?.unwrap_or_else(||package_digest.clone());
                    let current = selected_revision(&connection, &agent, &owner)?;
                    if current.as_ref().is_some_and(|current| current.package_digest != *package_digest) {
                        return Err(PluginError::Conflict("this plugin identity now selects another revision; use enable_plugin or explicit replace_plugin".to_owned()));
                    }
                    let existing = revision_owner(&connection,&agent,package_digest)?;
                    (existing.unwrap_or_else(|| package_digest.clone()), package_digest.clone(), None, true)
                }
                ActivationChange::Replace { plugin_id, package_digest, expected_digest } => {
                    super::store::validate_digest(expected_digest)?;
                    (plugin_id.clone(),package_digest.clone(),Some(expected_digest.clone()),true)
                }
                ActivationChange::Enable { plugin_id } | ActivationChange::Deactivate { plugin_id } => {
                    let current = selected_revision(&connection, &agent, plugin_id)?.ok_or_else(|| PluginError::NotFound("plugin activation was not found for this agent".to_owned()))?;
                    (plugin_id.clone(),current.package_digest.clone(),Some(current.package_digest),matches!(change,ActivationChange::Enable { .. }))
                }
            };
            super::store::validate_digest(&plugin_id)?;
            super::store::validate_digest(&digest)?;
            let prepared = if enabled {
                let root = store.package_root(&digest)?;
                Some(crate::skills::SkillStore::prepare_plugin(&plugin_id, &root)?)
            } else { None };
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(result) = receipt(&tx, &agent, &operation_id, &request)? {
                return Ok(result);
            }
            let current = selected_revision(&tx, &agent, &plugin_id)?;
            match (&expected, &current) {
                (Some(expected), Some(current)) if expected == &current.package_digest => (),
                (None, None) => (),
                (None, Some(current)) if current.package_digest == digest => (),
                _ => return Err(PluginError::Conflict("plugin revision changed; inspect its activation and copy the current package_digest before replacing it".to_owned())),
            }
            let occupied = revision_owner(&tx,&agent,&digest)?.filter(|owner|*owner!=plugin_id);
            if occupied.is_some() {
                return Err(PluginError::Conflict("that revision already has a different activation identity for this agent".to_owned()));
            }
            tx.execute("INSERT INTO host_agent_plugins(agent_id,plugin_id,package_digest,enabled) VALUES (?1,?2,?3,?4)
                ON CONFLICT(agent_id,plugin_id) DO UPDATE SET package_digest=excluded.package_digest,enabled=excluded.enabled",params![agent,plugin_id,digest,enabled])?;
            tx.execute("INSERT OR IGNORE INTO host_agent_plugin_revisions(agent_id,plugin_id,package_digest) VALUES (?1,?2,?3)",params![agent,plugin_id,digest])?;
            if enabled { super::store::record_admission(&tx, &digest)?; }
            let imported = prepared.as_ref().map(|prepared|skills.commit_plugin(&tx,&agent,prepared)).transpose()?;
            let (report, publications) = imported.map_or((None,None), |(report,publications)|(Some(report),Some(publications)));
            let result = PluginActivation { plugin_id, package_digest: digest, enabled, session_instructions: "pinned_until_session_end".to_owned(), skills: report };
            tx.execute("INSERT INTO host_plugin_activation_operations(agent_id,operation_id,request_json,result_json) VALUES (?1,?2,?3,?4)",params![agent,operation_id,request,serde_json::to_string(&result)?])?;
            if let Some(publications) = publications { publications.commit(tx)?; } else { tx.commit()?; }
            Ok(result)
        }).await?
    }

    #[cfg(test)]
    pub(crate) async fn activations(
        &self,
        agent: &renoa_kernel::AgentId,
    ) -> Result<Vec<PluginActivation>, PluginError> {
        Ok(self.agent_snapshot(agent).await?.activations)
    }

    pub(super) async fn activate_added(
        &self,
        agent: &renoa_kernel::AgentId,
        digest: &str,
        operation_id: &str,
    ) -> Result<(PluginActivation, SkillComponentReport), PluginError> {
        let activation = self
            .change_activation(
                agent,
                ActivationChange::Activate {
                    package_digest: digest.to_owned(),
                },
                operation_id,
            )
            .await?;
        let report = activation
            .skills
            .clone()
            .unwrap_or_else(|| SkillComponentReport::new(Vec::new(), Vec::new()));
        Ok((activation, report))
    }
}

fn receipt(
    connection: &Connection,
    agent: &str,
    operation: &str,
    request: &str,
) -> Result<Option<PluginActivation>, PluginError> {
    let receipt = connection.query_row("SELECT request_json,result_json FROM host_plugin_activation_operations WHERE agent_id=?1 AND operation_id=?2",params![agent,operation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?;
    match receipt {
        Some((stored, result)) if stored == request => Ok(Some(serde_json::from_str(&result)?)),
        Some(_) => Err(PluginError::Conflict(
            "activation operation identity was already used for different fields".to_owned(),
        )),
        None => Ok(None),
    }
}

fn selected_revision(
    connection: &Connection,
    agent: &str,
    plugin: &str,
) -> Result<Option<PluginActivation>, PluginError> {
    Ok(connection.query_row("SELECT plugin_id,package_digest,enabled FROM host_agent_plugins WHERE agent_id=?1 AND plugin_id=?2",params![agent,plugin],row).optional()?)
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginActivation> {
    Ok(PluginActivation {
        plugin_id: row.get(0)?,
        package_digest: row.get(1)?,
        enabled: row.get(2)?,
        session_instructions: "pinned_until_session_end".to_owned(),
        skills: None,
    })
}

/// Selecting an MCP account establishes its revision's activation when none
/// exists. An explicit deactivation or replacement cannot be bypassed by a
/// later connection selection. This does not import skill instructions.
pub(crate) fn admit_connection_selection(
    tx: &rusqlite::Transaction<'_>,
    agent: &str,
    connection: &str,
) -> Result<(), crate::mcp::McpHostError> {
    let digest = tx.query_row("SELECT server.plugin_digest FROM plugin_mcp_servers AS server JOIN mcp_connections AS connection ON connection.integration_id=server.integration_id WHERE connection.connection_id=?1",[connection],|r|r.get::<_,String>(0)).optional()?;
    let Some(digest) = digest else {
        return Ok(());
    };
    require_revision_selection(tx, agent, &digest)?;
    let current = tx.query_row("SELECT plugin_id,enabled FROM host_agent_plugins WHERE agent_id=?1 AND package_digest=?2",params![agent,digest],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?))).optional()?;
    match current {
        Some((_, true)) => Ok(()),
        Some((plugin, false)) => Err(crate::mcp::McpHostError::Conflict(format!(
            "plugin '{plugin}' is disabled; use plugin_manage enable_plugin before selecting this connection"
        ))),
        None => {
            let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM host_agent_plugin_revisions WHERE agent_id=?1 AND package_digest=?2)",params![agent,digest],|r|r.get(0))?;
            if occupied {
                return Err(crate::mcp::McpHostError::Conflict("this plugin identity now selects a replacement revision; use explicit replace_plugin to select the old revision".to_owned()));
            }
            tx.execute("INSERT INTO host_agent_plugins(agent_id,plugin_id,package_digest,enabled) VALUES (?1,?2,?2,1)",params![agent,digest])?;
            tx.execute("INSERT INTO host_agent_plugin_revisions(agent_id,plugin_id,package_digest) VALUES (?1,?2,?2)",params![agent,digest])?;
            Ok(())
        }
    }
}

fn require_revision_selection(
    connection: &Connection,
    agent: &str,
    digest: &str,
) -> Result<(), crate::mcp::McpHostError> {
    let admitted: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_plugin_admissions WHERE package_digest=?1)",
        [digest],
        |row| row.get(0),
    )?;
    if !admitted {
        return Err(crate::mcp::McpHostError::Conflict("this retained plugin revision needs current Host validation; explicitly install or activate it before selecting or authorizing its accounts".to_owned()));
    }
    let current=connection.query_row("SELECT plugin.package_digest,plugin.enabled FROM host_agent_plugin_revisions AS revision JOIN host_agent_plugins AS plugin ON plugin.agent_id=revision.agent_id AND plugin.plugin_id=revision.plugin_id WHERE revision.agent_id=?1 AND revision.package_digest=?2",params![agent,digest],|row|Ok((row.get::<_,String>(0)?,row.get::<_,bool>(1)?))).optional()?;
    match current {
        Some((selected,true)) if selected==digest => Ok(()),
        Some((selected,_)) if selected!=digest => Err(crate::mcp::McpHostError::Conflict("this plugin revision was replaced; use explicit replace_plugin before connecting its accounts".to_owned())),
        Some(_) => Err(crate::mcp::McpHostError::Conflict("plugin is disabled; use enable_plugin before connecting or authorizing its accounts".to_owned())),
        None => Ok(()),
    }
}

fn revision_owner(
    connection: &Connection,
    agent: &str,
    digest: &str,
) -> Result<Option<String>, PluginError> {
    Ok(connection.query_row("SELECT plugin_id FROM host_agent_plugin_revisions WHERE agent_id=?1 AND package_digest=?2",params![agent,digest],|r|r.get(0)).optional()?)
}

pub(crate) fn read_activations(
    connection: &Connection,
    agent: &str,
) -> Result<Vec<PluginActivation>, PluginError> {
    let mut q=connection.prepare("SELECT plugin_id,package_digest,enabled FROM host_agent_plugins WHERE agent_id=?1 ORDER BY plugin_id")?;
    Ok(q.query_map([agent], row)?.collect::<Result<Vec<_>, _>>()?)
}
