use rusqlite::{Connection, OptionalExtension as _};
use serde_json::Value;

use super::super::{
    AdapterCatalog, McpCatalogSnapshot, McpCatalogTool, McpHostError, McpRejectedTool,
};

pub(super) fn load_catalog(
    connection: &Connection,
    connection_id: &str,
) -> Result<Option<McpCatalogSnapshot>, McpHostError> {
    let metadata = connection
        .query_row(
            "SELECT endpoint, protocol_version, adapter_revision, catalog_digest
             FROM mcp_catalogs WHERE connection_id = ?1",
            [connection_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((endpoint, protocol_version, adapter_revision, digest)) = metadata else {
        return Ok(None);
    };
    let tools = load_tools(connection, connection_id)?;
    let mut statement = connection.prepare(
        "SELECT source_index, name, reason
         FROM mcp_rejected_tools WHERE connection_id = ?1 ORDER BY source_index",
    )?;
    let rejected_tools = statement
        .query_map([connection_id], |row| {
            Ok(McpRejectedTool {
                index: usize::try_from(row.get::<_, i64>(0)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?,
                name: row.get(1)?,
                reason: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = McpCatalogSnapshot::from_adapter(
        connection_id,
        AdapterCatalog {
            endpoint,
            protocol_version,
            adapter_revision,
            tools,
            rejected_tools,
        },
    )?;
    if snapshot.digest() != digest {
        return Err(McpHostError::Invalid(format!(
            "stored catalog digest for connection '{connection_id}' does not match its contents"
        )));
    }
    Ok(Some(snapshot))
}

fn load_tools(
    connection: &Connection,
    connection_id: &str,
) -> Result<Vec<McpCatalogTool>, McpHostError> {
    let mut statement = connection.prepare(
        "SELECT name, description, input_schema_json, model_input_schema_json, output_schema_json
         FROM mcp_tools WHERE connection_id = ?1 ORDER BY name",
    )?;
    statement
        .query_map([connection_id], |row| {
            let output = row.get::<_, Option<String>>(4)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                output,
            ))
        })?
        .map(|row| {
            let (name, description, input, model_input, output) = row?;
            Ok(McpCatalogTool {
                name,
                description,
                input_schema: parse_object(&input, "input")?,
                model_input_schema: parse_object(&model_input, "model input")?,
                output_schema: output
                    .as_deref()
                    .map(|encoded| parse_object(encoded, "output"))
                    .transpose()?,
            })
        })
        .collect()
}

fn parse_object(encoded: &str, kind: &str) -> Result<Value, McpHostError> {
    let value: Value = serde_json::from_str(encoded)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(McpHostError::Invalid(format!(
            "stored {kind} tool schema is not an object"
        )))
    }
}
