//! Canonical agent-definition storage.
//!
//! One root row owns identity, creation provenance, the preset id, and the
//! complete core operational document. Normalized children own the exact tool
//! selection and the selected Host connections. Typed receipts make creation,
//! rename, and selection edits idempotent.

use rusqlite::Transaction;

use super::catalog::HostCatalogError;

pub(in crate::host) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_agents (
            agent_id TEXT PRIMARY KEY CHECK (length(agent_id) = 36),
            name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 512),
            created_at_ms INTEGER NOT NULL,
            created_via TEXT NOT NULL CHECK (
                created_via IN ('agent_tool', 'management', 'provisioning')
            ),
            preset_id TEXT CHECK (
                preset_id IS NULL OR length(preset_id) BETWEEN 1 AND 128
            ),
            operational_json TEXT NOT NULL CHECK (json_valid(operational_json)),
            creator_kind TEXT NOT NULL CHECK (
                creator_kind IN ('agent', 'principal', 'system')
            ),
            creator_agent_id TEXT REFERENCES host_agents(agent_id),
            creator_host_id TEXT,
            creator_principal_id TEXT,
            creator_component TEXT,
            CHECK (
                (creator_kind = 'agent'
                    AND creator_agent_id IS NOT NULL
                    AND creator_host_id IS NULL
                    AND creator_principal_id IS NULL
                    AND creator_component IS NULL)
                OR (creator_kind = 'principal'
                    AND creator_agent_id IS NULL
                    AND creator_host_id IS NOT NULL
                    AND creator_principal_id IS NOT NULL
                    AND creator_component IS NULL)
                OR (creator_kind = 'system'
                    AND creator_agent_id IS NULL
                    AND creator_host_id IS NULL
                    AND creator_principal_id IS NULL
                    AND creator_component IS NOT NULL)
            ),
            CHECK (creator_agent_id IS NULL OR creator_agent_id != agent_id)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_agent_tool_selections (
            agent_id TEXT PRIMARY KEY
                REFERENCES host_agents(agent_id),
            revision INTEGER NOT NULL CHECK (revision > 0),
            tools_json TEXT NOT NULL CHECK (json_valid(tools_json))
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_agent_mcp_connections (
            agent_id TEXT NOT NULL
                REFERENCES host_agents(agent_id),
            connection_id TEXT NOT NULL
                REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
            PRIMARY KEY (agent_id, connection_id)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_agent_creations (
            operation_id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            request_json TEXT NOT NULL CHECK (json_valid(request_json))
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_agent_tool_selection_operations (
            operation_id TEXT PRIMARY KEY,
            request_json TEXT NOT NULL CHECK (json_valid(request_json)),
            result_json TEXT NOT NULL CHECK (json_valid(result_json))
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_agent_renames (
            operation_id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            actor_agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            request_json TEXT NOT NULL CHECK (json_valid(request_json)),
            result_json TEXT NOT NULL CHECK (json_valid(result_json))
        ) STRICT;",
    )?;
    Ok(())
}
