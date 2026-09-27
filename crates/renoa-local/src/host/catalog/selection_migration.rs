//! Catalog upgrades retain machine grants; plugin access has its own owner.

use std::collections::BTreeSet;

use rusqlite::{Transaction, params};

use super::HostCatalogError;
use crate::{AgentToolSelection, capabilities, plugins::host::HostPluginId};

pub(super) fn migrate(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    let mut statement = transaction
        .prepare("SELECT agent_id, revision, tools_json FROM host_agent_tool_selections")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (agent, revision, encoded) in rows {
        let context = format!("tool selection for agent {agent}");
        let mut selection = AgentToolSelection {
            revision,
            tools: serde_json::from_str(&encoded).map_err(|error| invalid(&context, error))?,
        };
        if normalize(&mut selection, &context)? {
            transaction.execute(
                "UPDATE host_agent_tool_selections SET revision=?2, tools_json=?3 WHERE agent_id=?1",
                params![agent, selection.revision,
                    serde_json::to_string(&selection.tools).map_err(|error| invalid(&context, error))?],
            )?;
        }
    }
    for (table, pointer) in [
        ("host_agent_creations", "/tool_selection"),
        ("host_agent_renames", "/tool_selection"),
        ("host_agent_tool_selection_operations", ""),
    ] {
        migrate_receipts(transaction, table, pointer)?;
    }
    Ok(())
}

fn migrate_receipts(
    transaction: &Transaction<'_>,
    table: &str,
    pointer: &str,
) -> Result<(), HostCatalogError> {
    let mut statement =
        transaction.prepare(&format!("SELECT operation_id, result_json FROM {table}"))?;
    let receipts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (operation, encoded) in receipts {
        let context = format!("{table} receipt {operation}");
        let mut result: serde_json::Value =
            serde_json::from_str(&encoded).map_err(|error| invalid(&context, error))?;
        let value = result
            .pointer_mut(pointer)
            .ok_or_else(|| invalid(&context, "missing tool selection"))?;
        let mut selection: AgentToolSelection =
            serde_json::from_value(value.clone()).map_err(|error| invalid(&context, error))?;
        if normalize(&mut selection, &context)? {
            *value = serde_json::to_value(selection).map_err(|error| invalid(&context, error))?;
            transaction.execute(
                &format!("UPDATE {table} SET result_json=?2 WHERE operation_id=?1"),
                params![
                    operation,
                    serde_json::to_string(&result).map_err(|error| invalid(&context, error))?
                ],
            )?;
        }
    }
    Ok(())
}

fn normalize(selection: &mut AgentToolSelection, context: &str) -> Result<bool, HostCatalogError> {
    if selection.revision <= 0 {
        return Err(invalid(context, "tool selection revision must be positive"));
    }
    let mut machine = BTreeSet::new();
    for name in &selection.tools {
        if capabilities::is_selectable(name) {
            machine.insert(name.clone());
        } else if !matches!(
            name.as_str(),
            "extension_manage"
                | "tool_search"
                | "tool_load"
                | "code_mode"
                | capabilities::PLUGIN_MANAGE
                | capabilities::PLUGIN_SEARCH
                | capabilities::TOOL_EXECUTE
        ) && HostPluginId::owner(name).is_none()
        {
            return Err(invalid(context, format!("unrecognized capability: {name}")));
        }
    }
    if machine == selection.tools {
        return Ok(false);
    }
    selection.revision = selection
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid(context, "tool selection revision overflow"))?;
    selection.tools = machine;
    Ok(true)
}

fn invalid(context: &str, reason: impl std::fmt::Display) -> HostCatalogError {
    HostCatalogError::Invalid(format!(
        "cannot migrate {context}: {reason}; use the explicit Host reset"
    ))
}
