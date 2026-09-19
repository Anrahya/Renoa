//! SQL access for the canonical agent-definition root and its children.

use std::collections::BTreeSet;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use uuid::Uuid;

use super::super::catalog::HostCatalogError;
use crate::{
    AgentCreationOrigin, AgentCreator, AgentDefinition, AgentOperationalDefinition, AgentPresetId,
    AgentToolSelection,
};

/// One persisted creation receipt.
///
/// The receipt proves which operation created which agent, what request it
/// carried, and the definition its commit produced. The root row stays the
/// single durable owner of the live definition, so replay reads this immutable
/// result instead of the current state.
pub(super) struct CreationReceipt {
    pub(super) agent_id: AgentId,
    pub(super) request_json: String,
    pub(super) result_json: String,
}

/// One persisted tool-selection receipt.
pub(super) struct SelectionReceipt {
    pub(super) request_json: String,
    pub(super) result_json: String,
}

pub(super) fn insert(
    transaction: &Transaction<'_>,
    definition: &AgentDefinition,
) -> Result<(), HostCatalogError> {
    let (kind, agent_id, host_id, principal_id, component) = encode_creator(&definition.creator);
    transaction.execute(
        "INSERT INTO host_agents(
            agent_id, name, created_at_ms, created_via, preset_id, operational_json,
            creator_kind, creator_agent_id, creator_host_id, creator_principal_id,
            creator_component
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            definition.id.to_string(),
            definition.name,
            definition.created_at_ms,
            definition.created_via.as_str(),
            definition.preset_id.as_ref().map(AgentPresetId::as_str),
            serde_json::to_string(&definition.operational).map_err(|error| invalid_json(&error))?,
            kind,
            agent_id,
            host_id,
            principal_id,
            component,
        ],
    )?;
    write_selection(transaction, definition.id, &definition.tool_selection)?;
    set_connections(transaction, definition.id, &definition.connections)?;
    Ok(())
}

pub(super) fn insert_creation_receipt(
    transaction: &Transaction<'_>,
    operation: Uuid,
    agent: AgentId,
    request_json: &str,
    result_json: &str,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "INSERT INTO host_agent_creations(operation_id, agent_id, request_json, result_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            operation.to_string(),
            agent.to_string(),
            request_json,
            result_json
        ],
    )?;
    Ok(())
}

pub(super) fn creation_receipt(
    connection: &Connection,
    operation: Uuid,
) -> Result<Option<CreationReceipt>, HostCatalogError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT agent_id, request_json, result_json FROM host_agent_creations
             WHERE operation_id = ?1",
            [operation.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((agent_id, request_json, result_json)) = row else {
        return Ok(None);
    };
    Ok(Some(CreationReceipt {
        agent_id: parse_agent(&agent_id)?,
        request_json,
        result_json,
    }))
}

/// One `host_agents` row, named so the eleven decoded columns cannot be
/// transposed positionally.
struct StoredRow {
    agent_id: String,
    name: String,
    created_at_ms: i64,
    created_via: String,
    preset_id: Option<String>,
    operational_json: String,
    creator_kind: String,
    creator_agent_id: Option<String>,
    creator_host_id: Option<String>,
    creator_principal_id: Option<String>,
    creator_component: Option<String>,
}

impl StoredRow {
    fn read(row: &rusqlite::Row<'_>) -> Result<Self, rusqlite::Error> {
        Ok(Self {
            agent_id: row.get(0)?,
            name: row.get(1)?,
            created_at_ms: row.get(2)?,
            created_via: row.get(3)?,
            preset_id: row.get(4)?,
            operational_json: row.get(5)?,
            creator_kind: row.get(6)?,
            creator_agent_id: row.get(7)?,
            creator_host_id: row.get(8)?,
            creator_principal_id: row.get(9)?,
            creator_component: row.get(10)?,
        })
    }
}

pub(super) fn read(
    connection: &Connection,
    agent: AgentId,
) -> Result<Option<AgentDefinition>, HostCatalogError> {
    let row = connection
        .query_row(
            "SELECT agent_id, name, created_at_ms, created_via, preset_id, operational_json,
                        creator_kind, creator_agent_id, creator_host_id, creator_principal_id,
                        creator_component
                 FROM host_agents WHERE agent_id = ?1",
            [agent.to_string()],
            StoredRow::read,
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id = parse_agent(&row.agent_id)?;
    let operational: AgentOperationalDefinition =
        serde_json::from_str(&row.operational_json).map_err(|error| invalid_json(&error))?;
    let definition = AgentDefinition {
        id,
        name: row.name,
        created_at_ms: row.created_at_ms,
        creator: decode_creator(
            &row.creator_kind,
            row.creator_agent_id.as_deref(),
            row.creator_host_id.as_deref(),
            row.creator_principal_id.as_deref(),
            row.creator_component.as_deref(),
        )?,
        created_via: parse_origin(&row.created_via)?,
        preset_id: row
            .preset_id
            .map(|id| {
                AgentPresetId::new(id).map_err(|error| HostCatalogError::Invalid(error.to_string()))
            })
            .transpose()?,
        operational,
        tool_selection: read_selection(connection, id)?,
        connections: read_connections(connection, id)?,
    };
    // Stored state must satisfy the rules every writer applies: a row written by
    // an older runtime or edited outside the Host fails closed here instead of
    // reaching the runtime.
    require_stored_definition(&definition)?;
    Ok(Some(definition))
}

/// Rejects stored state no writer can produce: a definition that fails the
/// writer-side validation, or a selection naming a capability the definition
/// cannot exercise.
fn require_stored_definition(definition: &AgentDefinition) -> Result<(), HostCatalogError> {
    definition.validate().map_err(|error| {
        HostCatalogError::Invalid(format!(
            "stored agent definition for `{}` is invalid: {error}",
            definition.id
        ))
    })?;
    for name in &definition.tool_selection.tools {
        if !crate::capabilities::is_selectable(name)
            || !crate::capabilities::is_consumable(name, definition.operational.documents)
        {
            return Err(HostCatalogError::Invalid(format!(
                "stored agent definition for `{}` names an unusable capability: {name}",
                definition.id
            )));
        }
    }
    Ok(())
}

pub(super) fn list(
    connection: &Connection,
    after: Option<AgentId>,
    limit: usize,
) -> Result<Vec<AgentDefinition>, HostCatalogError> {
    let mut statement = connection.prepare(
        "SELECT agent_id FROM host_agents
         WHERE agent_id > ?1 ORDER BY agent_id LIMIT ?2",
    )?;
    let ids = statement
        .query_map(
            params![
                after.map(|id| id.to_string()).unwrap_or_default(),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut definitions = Vec::with_capacity(ids.len());
    for id in ids {
        let id = parse_agent(&id)?;
        if let Some(definition) = read(connection, id)? {
            definitions.push(definition);
        }
    }
    Ok(definitions)
}

pub(super) fn exists(connection: &Connection, agent: AgentId) -> Result<bool, HostCatalogError> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM host_agents WHERE agent_id = ?1",
            [agent.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub(super) fn read_selection(
    connection: &Connection,
    agent: AgentId,
) -> Result<AgentToolSelection, HostCatalogError> {
    let row: Option<(i64, String)> = connection
        .query_row(
            "SELECT revision, tools_json FROM host_agent_tool_selections WHERE agent_id = ?1",
            [agent.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((revision, tools)) = row else {
        return Err(HostCatalogError::Invalid(format!(
            "agent `{agent}` has no stored tool selection"
        )));
    };
    Ok(AgentToolSelection {
        revision,
        tools: serde_json::from_str(&tools).map_err(|error| invalid_json(&error))?,
    })
}

pub(super) fn write_selection(
    transaction: &Transaction<'_>,
    agent: AgentId,
    selection: &AgentToolSelection,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "INSERT INTO host_agent_tool_selections(agent_id, revision, tools_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(agent_id) DO UPDATE SET
             revision = excluded.revision,
             tools_json = excluded.tools_json",
        params![
            agent.to_string(),
            selection.revision,
            serde_json::to_string(&selection.tools).map_err(|error| invalid_json(&error))?
        ],
    )?;
    Ok(())
}

pub(super) fn selection_receipt(
    connection: &Connection,
    operation: Uuid,
) -> Result<Option<SelectionReceipt>, HostCatalogError> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT request_json, result_json FROM host_agent_tool_selection_operations
             WHERE operation_id = ?1",
            [operation.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(request_json, result_json)| SelectionReceipt {
        request_json,
        result_json,
    }))
}

pub(super) fn insert_selection_receipt(
    transaction: &Transaction<'_>,
    operation: Uuid,
    request_json: &str,
    result_json: &str,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "INSERT INTO host_agent_tool_selection_operations(operation_id, request_json, result_json)
         VALUES (?1, ?2, ?3)",
        params![operation.to_string(), request_json, result_json],
    )?;
    Ok(())
}

pub(super) fn read_connections(
    connection: &Connection,
    agent: AgentId,
) -> Result<BTreeSet<String>, HostCatalogError> {
    let mut statement = connection.prepare(
        "SELECT connection_id FROM host_agent_mcp_connections
         WHERE agent_id = ?1 ORDER BY connection_id",
    )?;
    let connections = statement
        .query_map([agent.to_string()], |row| row.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(connections)
}

pub(super) fn set_connections(
    transaction: &Transaction<'_>,
    agent: AgentId,
    connections: &BTreeSet<String>,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "DELETE FROM host_agent_mcp_connections WHERE agent_id = ?1",
        [agent.to_string()],
    )?;
    for connection_id in connections {
        transaction.execute(
            "INSERT INTO host_agent_mcp_connections(agent_id, connection_id) VALUES (?1, ?2)",
            params![agent.to_string(), connection_id],
        )?;
    }
    Ok(())
}

pub(super) fn set_name(
    transaction: &Transaction<'_>,
    agent: AgentId,
    name: &str,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "UPDATE host_agents SET name = ?2 WHERE agent_id = ?1",
        params![agent.to_string(), name],
    )?;
    Ok(())
}

pub(super) fn rename_receipt(
    connection: &Connection,
    operation: Uuid,
) -> Result<Option<(String, String, String)>, HostCatalogError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT actor_agent_id, request_json, result_json FROM host_agent_renames
             WHERE operation_id = ?1",
            [operation.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(row)
}

pub(super) fn insert_rename_receipt(
    transaction: &Transaction<'_>,
    operation: Uuid,
    agent: AgentId,
    actor: AgentId,
    request_json: &str,
    result_json: &str,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "INSERT INTO host_agent_renames(
            operation_id, agent_id, actor_agent_id, request_json, result_json
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            operation.to_string(),
            agent.to_string(),
            actor.to_string(),
            request_json,
            result_json
        ],
    )?;
    Ok(())
}

fn encode_creator(
    creator: &AgentCreator,
) -> (
    &'static str,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    match creator {
        AgentCreator::Agent { agent_id } => ("agent", Some(agent_id.to_string()), None, None, None),
        AgentCreator::Principal {
            host_id,
            principal_id,
        } => (
            "principal",
            None,
            Some(host_id.to_string()),
            Some(principal_id.clone()),
            None,
        ),
        AgentCreator::System { component } => ("system", None, None, None, Some(component.clone())),
    }
}

fn decode_creator(
    kind: &str,
    agent_id: Option<&str>,
    host_id: Option<&str>,
    principal_id: Option<&str>,
    component: Option<&str>,
) -> Result<AgentCreator, HostCatalogError> {
    match (kind, agent_id, host_id, principal_id, component) {
        ("agent", Some(agent_id), None, None, None) => Ok(AgentCreator::Agent {
            agent_id: parse_agent(agent_id)?,
        }),
        ("principal", None, Some(host_id), Some(principal_id), None) => {
            Ok(AgentCreator::Principal {
                host_id: Uuid::parse_str(host_id).map_err(|error| {
                    HostCatalogError::Invalid(format!("invalid creator host identity: {error}"))
                })?,
                principal_id: principal_id.to_owned(),
            })
        }
        ("system", None, None, None, Some(component)) => Ok(AgentCreator::System {
            component: component.to_owned(),
        }),
        _ => Err(HostCatalogError::Invalid(
            "stored agent creator does not match a single valid variant".to_owned(),
        )),
    }
}

fn parse_origin(value: &str) -> Result<AgentCreationOrigin, HostCatalogError> {
    match value {
        "agent_tool" => Ok(AgentCreationOrigin::AgentTool),
        "management" => Ok(AgentCreationOrigin::Management),
        "provisioning" => Ok(AgentCreationOrigin::Provisioning),
        other => Err(HostCatalogError::Invalid(format!(
            "stored agent creation origin `{other}` is not supported"
        ))),
    }
}

fn parse_agent(value: &str) -> Result<AgentId, HostCatalogError> {
    Uuid::parse_str(value)
        .map(AgentId::from_uuid)
        .map_err(|error| {
            HostCatalogError::Invalid(format!("invalid stored agent identity: {error}"))
        })
}

fn invalid_json(error: &serde_json::Error) -> HostCatalogError {
    HostCatalogError::Invalid(format!("invalid stored agent definition: {error}"))
}
