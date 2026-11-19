//! Schema 57 adds native stdio MCP transport storage to `mcp_integrations`
//! (`kind = 'direct_stdio'`) and `plugin_mcp_servers` (`transport = 'stdio'`).

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn upgrade(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "DROP VIEW IF EXISTS host_agent_enabled_mcp_connections;

        CREATE TABLE mcp_integrations_v57 (
            integration_id TEXT PRIMARY KEY CHECK (length(integration_id) > 0),
            kind TEXT NOT NULL CHECK (kind IN ('direct_streamable_http', 'direct_stdio')),
            endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
            request_headers_json TEXT NOT NULL CHECK (
                json_valid(request_headers_json)
                AND json_type(request_headers_json) = 'object'
            ),
            stdio_config_json TEXT,
            CHECK (
                (kind = 'direct_streamable_http' AND stdio_config_json IS NULL)
                OR
                (kind = 'direct_stdio'
                 AND length(endpoint) = 70
                 AND substr(endpoint, 1, 6) = 'stdio:'
                 AND request_headers_json = '{}'
                 AND stdio_config_json IS NOT NULL
                 AND json_valid(stdio_config_json)
                 AND json_type(stdio_config_json) = 'object')
            )
        ) STRICT;

        INSERT INTO mcp_integrations_v57(
            integration_id, kind, endpoint, request_headers_json, stdio_config_json
        )
        SELECT integration_id, kind, endpoint, request_headers_json, NULL
        FROM mcp_integrations;

        DROP TABLE mcp_integrations;
        ALTER TABLE mcp_integrations_v57 RENAME TO mcp_integrations;

        CREATE TABLE plugin_mcp_servers_v57 (
            plugin_digest TEXT NOT NULL
                REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
            server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
            integration_id TEXT NOT NULL UNIQUE,
            transport TEXT NOT NULL CHECK (transport IN ('streamable_http', 'stdio')),
            endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
            request_headers_json TEXT NOT NULL CHECK (
                json_valid(request_headers_json)
                AND json_type(request_headers_json) = 'object'
            ),
            stdio_config_json TEXT,
            CHECK (
                (transport = 'streamable_http' AND stdio_config_json IS NULL)
                OR
                (transport = 'stdio'
                 AND length(endpoint) = 70
                 AND substr(endpoint, 1, 6) = 'stdio:'
                 AND request_headers_json = '{}'
                 AND stdio_config_json IS NOT NULL
                 AND json_valid(stdio_config_json)
                 AND json_type(stdio_config_json) = 'object')
            ),
            PRIMARY KEY (plugin_digest, server_id)
        ) STRICT;

        INSERT INTO plugin_mcp_servers_v57(
            plugin_digest, server_id, integration_id, transport, endpoint,
            request_headers_json, stdio_config_json
        )
        SELECT plugin_digest, server_id, integration_id, transport, endpoint,
               request_headers_json, NULL
        FROM plugin_mcp_servers;

        DROP TABLE plugin_mcp_servers;
        ALTER TABLE plugin_mcp_servers_v57 RENAME TO plugin_mcp_servers;",
    )?;
    crate::plugins::activation::schema::initialize(transaction)?;
    Ok(())
}

/// Turns a schema 57 catalog back into schema 56's tables.
#[cfg(test)]
pub(crate) fn restore_schema_56(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             PRAGMA legacy_alter_table = ON;
             DELETE FROM host_agent_mcp_connections
             WHERE connection_id IN (
                 SELECT connection_id FROM mcp_connections
                 WHERE integration_id IN (
                     SELECT integration_id FROM mcp_integrations
                     WHERE kind != 'direct_streamable_http'
                 )
             );
             DELETE FROM mcp_tools
             WHERE connection_id IN (
                 SELECT connection_id FROM mcp_connections
                 WHERE integration_id IN (
                     SELECT integration_id FROM mcp_integrations
                     WHERE kind != 'direct_streamable_http'
                 )
             );
             DELETE FROM mcp_rejected_tools
             WHERE connection_id IN (
                 SELECT connection_id FROM mcp_connections
                 WHERE integration_id IN (
                     SELECT integration_id FROM mcp_integrations
                     WHERE kind != 'direct_streamable_http'
                 )
             );
             DELETE FROM mcp_catalogs
             WHERE connection_id IN (
                 SELECT connection_id FROM mcp_connections
                 WHERE integration_id IN (
                     SELECT integration_id FROM mcp_integrations
                     WHERE kind != 'direct_streamable_http'
                 )
             );
             DELETE FROM mcp_connections
             WHERE integration_id IN (
                 SELECT integration_id FROM mcp_integrations
                 WHERE kind != 'direct_streamable_http'
             );
             DELETE FROM plugin_mcp_servers WHERE transport != 'streamable_http';
             DELETE FROM mcp_integrations WHERE kind != 'direct_streamable_http';
             CREATE TABLE mcp_integrations_v56 (
                 integration_id TEXT PRIMARY KEY CHECK (length(integration_id) > 0),
                 kind TEXT NOT NULL CHECK (kind = 'direct_streamable_http'),
                 endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
                 request_headers_json TEXT NOT NULL CHECK (
                     json_valid(request_headers_json)
                     AND json_type(request_headers_json) = 'object'
                 )
             ) STRICT;
             INSERT INTO mcp_integrations_v56(
                 integration_id, kind, endpoint, request_headers_json
             )
             SELECT integration_id, kind, endpoint, request_headers_json
             FROM mcp_integrations;
             DROP TABLE mcp_integrations;
             ALTER TABLE mcp_integrations_v56 RENAME TO mcp_integrations;

             CREATE TABLE plugin_mcp_servers_v56 (
                 plugin_digest TEXT NOT NULL
                     REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
                 server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
                 integration_id TEXT NOT NULL UNIQUE,
                 transport TEXT NOT NULL CHECK (transport = 'streamable_http'),
                 endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
                 request_headers_json TEXT NOT NULL CHECK (
                     json_valid(request_headers_json)
                     AND json_type(request_headers_json) = 'object'
                 ),
                 PRIMARY KEY (plugin_digest, server_id)
             ) STRICT;
             INSERT INTO plugin_mcp_servers_v56(
                 plugin_digest, server_id, integration_id, transport, endpoint,
                 request_headers_json
             )
             SELECT plugin_digest, server_id, integration_id, transport, endpoint,
                    request_headers_json
             FROM plugin_mcp_servers;
             DROP TABLE plugin_mcp_servers;
             ALTER TABLE plugin_mcp_servers_v56 RENAME TO plugin_mcp_servers;
             PRAGMA legacy_alter_table = OFF;
             PRAGMA foreign_keys = ON;",
        )
        .expect("restore schema 56");
}
