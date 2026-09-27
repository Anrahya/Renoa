use crate::host::catalog::HostCatalogError;
use rusqlite::Connection;

pub(crate) fn initialize(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_plugin_admissions (
            package_digest TEXT PRIMARY KEY REFERENCES installed_plugins(plugin_digest) ON DELETE CASCADE
         ) STRICT;
         CREATE TABLE IF NOT EXISTS host_agent_plugins (
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            plugin_id TEXT NOT NULL CHECK (length(plugin_id)=64),
            package_digest TEXT NOT NULL REFERENCES installed_plugins(plugin_digest),
            enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
            PRIMARY KEY(agent_id,plugin_id),
            UNIQUE(agent_id,package_digest)
         ) STRICT;
         CREATE TABLE IF NOT EXISTS host_agent_plugin_revisions (
            agent_id TEXT NOT NULL,
            plugin_id TEXT NOT NULL,
            package_digest TEXT NOT NULL REFERENCES installed_plugins(plugin_digest),
            FOREIGN KEY(agent_id,plugin_id) REFERENCES host_agent_plugins(agent_id,plugin_id),
            PRIMARY KEY(agent_id,package_digest)
         ) STRICT;
         CREATE TABLE IF NOT EXISTS host_plugin_activation_operations (
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            operation_id TEXT NOT NULL,
            request_json TEXT NOT NULL CHECK (json_valid(request_json)),
            result_json TEXT NOT NULL CHECK (json_valid(result_json)),
            PRIMARY KEY(agent_id,operation_id)
         ) STRICT;
         CREATE VIEW IF NOT EXISTS host_agent_enabled_mcp_connections AS
            SELECT binding.agent_id,binding.connection_id
            FROM host_agent_mcp_connections AS binding
            JOIN mcp_connections AS connection ON connection.connection_id=binding.connection_id
            LEFT JOIN plugin_mcp_servers AS server ON server.integration_id=connection.integration_id
            WHERE (server.plugin_digest IS NULL AND substr(connection.integration_id,1,7)!='plugin.') OR EXISTS (
                SELECT 1 FROM host_agent_plugins AS plugin
                WHERE plugin.agent_id=binding.agent_id AND plugin.enabled=1
                  AND plugin.package_digest=server.plugin_digest
                  AND EXISTS (SELECT 1 FROM host_plugin_admissions AS admission WHERE admission.package_digest=plugin.package_digest)
            );",
    )
}

pub(crate) fn initialize_lifecycle(
    tx: &rusqlite::Transaction<'_>,
    migrating: bool,
) -> Result<(), HostCatalogError> {
    if migrating {
        let legacy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_skill_bindings WHERE scope_kind='plugin') OR EXISTS(SELECT 1 FROM agent_skill_source_rejections WHERE scope_kind='plugin') OR EXISTS(SELECT 1 FROM host_agent_mcp_connections AS binding JOIN mcp_connections AS connection ON binding.connection_id=connection.connection_id WHERE substr(connection.integration_id,1,7)='plugin.')", [], |r|r.get(0))?;
        if legacy {
            return Err(HostCatalogError::Invalid("legacy plugin selections have no exact activation identity; use the explicit Host reset before starting this runtime".to_owned()));
        }
        let has_column = tx
            .prepare("PRAGMA table_info(plugin_mcp_servers)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|column| column == "integration_id");
        if !has_column {
            tx.execute_batch("ALTER TABLE plugin_mcp_servers ADD COLUMN integration_id TEXT;")?;
        }
        let servers = tx
            .prepare("SELECT plugin_digest,server_id FROM plugin_mcp_servers")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (digest, server) in servers {
            crate::plugins::store::validate_digest(&digest)
                .map_err(|error| HostCatalogError::Invalid(error.to_string()))?;
            tx.execute("UPDATE plugin_mcp_servers SET integration_id=?3 WHERE plugin_digest=?1 AND server_id=?2",rusqlite::params![digest,server,crate::plugins::manager::integration_id(&digest,&server)])?;
        }
        tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS plugin_mcp_integration_identity ON plugin_mcp_servers(integration_id);")?;
    }
    initialize(tx)?;
    crate::plugins::host::state::initialize(tx)?;
    crate::plugins::coherence::initialize(tx)?;
    Ok(())
}

pub(crate) fn verify_owners(
    connection: &Connection,
) -> Result<(), crate::host::catalog::HostCatalogError> {
    let mut q = connection
        .prepare("SELECT plugin_digest,server_id,integration_id FROM plugin_mcp_servers")?;
    let rows = q.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (digest, server, owner) = row?;
        crate::plugins::store::validate_digest(&digest)
            .map_err(|error| crate::host::catalog::HostCatalogError::Invalid(error.to_string()))?;
        if owner.as_deref()
            != Some(crate::plugins::manager::integration_id(&digest, &server).as_str())
        {
            return Err(crate::host::catalog::HostCatalogError::Invalid(
                "stored MCP integration owner differs from its plugin revision".to_owned(),
            ));
        }
    }
    let missing: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM host_agent_plugins AS plugin WHERE NOT EXISTS(SELECT 1 FROM host_agent_plugin_revisions AS revision WHERE revision.agent_id=plugin.agent_id AND revision.plugin_id=plugin.plugin_id AND revision.package_digest=plugin.package_digest))",[],|row|row.get(0))?;
    if missing {
        return Err(HostCatalogError::Invalid(
            "plugin activation has no matching revision identity".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
