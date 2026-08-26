use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};

use crate::mcp::McpHostError;

const SCHEMA_VERSION: u32 = 1;
pub(crate) const HOST_DATABASE: &str = "host.sqlite3";

const SCHEMA: &str = "
    CREATE TABLE host_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        schema_version INTEGER NOT NULL CHECK (schema_version > 0)
    ) STRICT;

    CREATE TABLE mcp_integrations (
        integration_id TEXT PRIMARY KEY CHECK (length(integration_id) > 0),
        kind TEXT NOT NULL CHECK (kind = 'direct_streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0)
    ) STRICT;

    CREATE TABLE mcp_connections (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (auth_kind = 'none')
    ) STRICT;

    CREATE TABLE mcp_catalogs (
        connection_id TEXT PRIMARY KEY
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        protocol_version TEXT NOT NULL CHECK (length(protocol_version) > 0),
        adapter_revision TEXT NOT NULL CHECK (length(adapter_revision) > 0),
        catalog_digest TEXT NOT NULL CHECK (length(catalog_digest) = 64)
    ) STRICT;

    CREATE TABLE mcp_tools (
        connection_id TEXT NOT NULL
            REFERENCES mcp_catalogs(connection_id) ON DELETE CASCADE,
        name TEXT NOT NULL CHECK (length(name) > 0),
        description TEXT NOT NULL,
        input_schema_json TEXT NOT NULL CHECK (json_valid(input_schema_json)),
        model_input_schema_json TEXT NOT NULL CHECK (json_valid(model_input_schema_json)),
        output_schema_json TEXT CHECK (
            output_schema_json IS NULL OR json_valid(output_schema_json)
        ),
        PRIMARY KEY (connection_id, name)
    ) STRICT;

    CREATE TABLE mcp_rejected_tools (
        connection_id TEXT NOT NULL
            REFERENCES mcp_catalogs(connection_id) ON DELETE CASCADE,
        source_index INTEGER NOT NULL CHECK (source_index >= 0),
        name TEXT,
        reason TEXT NOT NULL,
        PRIMARY KEY (connection_id, source_index)
    ) STRICT;

    CREATE TABLE profile_mcp_tools (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        connection_id TEXT NOT NULL
            REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
        tool_name TEXT NOT NULL CHECK (length(tool_name) > 0),
        PRIMARY KEY (profile_id, connection_id, tool_name)
    ) STRICT;

    INSERT INTO host_metadata(singleton, schema_version) VALUES (1, 1);
";

pub(super) fn open(path: &Path) -> Result<Connection, McpHostError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )?;
    Ok(connection)
}

pub(super) fn initialize(connection: &mut Connection) -> Result<(), McpHostError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version =
        transaction.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    match version {
        SCHEMA_VERSION => {
            transaction.commit()?;
            verify(connection)
        }
        0 => {
            transaction.execute_batch(SCHEMA)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            verify(connection)
        }
        found => Err(McpHostError::Invalid(format!(
            "Host catalog schema {found} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

pub(super) fn verify(connection: &Connection) -> Result<(), McpHostError> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    let metadata = connection
        .query_row(
            "SELECT schema_version FROM host_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, u32>(0),
        )
        .optional()?;
    if version != SCHEMA_VERSION || metadata != Some(SCHEMA_VERSION) {
        return Err(McpHostError::Invalid(
            "Host catalog metadata is missing or incompatible".to_owned(),
        ));
    }
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?;
    if violation.is_some() {
        return Err(McpHostError::Invalid(
            "Host catalog contains a foreign-key violation".to_owned(),
        ));
    }
    Ok(())
}
