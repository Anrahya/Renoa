use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};
use thiserror::Error;

mod agents;
mod cutover;
mod migrations;

pub(crate) use cutover::cutover_and_clear;
#[cfg(test)]
pub(crate) use cutover::{cutover, fail_next_clear_before_commit};

const SCHEMA_VERSION: u32 = 28;
pub(crate) const HOST_DATABASE: &str = "host.sqlite3";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HostCatalogError {
    #[error("Host catalog storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Host catalog database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid Host catalog: {0}")]
    Invalid(String),
}

const SCHEMA: &str = "
    CREATE TABLE host_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        schema_version INTEGER NOT NULL CHECK (schema_version > 0)
    ) STRICT;

    CREATE TABLE mcp_integrations (
        integration_id TEXT PRIMARY KEY CHECK (length(integration_id) > 0),
        kind TEXT NOT NULL CHECK (kind = 'direct_streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        )
    ) STRICT;

    CREATE TABLE mcp_connections (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (
            auth_kind IN (
                'none', 'gh_cli', 'secret_service_bearer',
                'secret_service_header', 'oauth'
            )
        ),
        auth_hostname TEXT,
        auth_account TEXT,
        auth_credential_id TEXT,
        oauth_registration_json TEXT,
        auth_header_name TEXT,
        auth_header_prefix TEXT,
        CHECK (
            (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL
             AND auth_credential_id IS NULL AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'gh_cli'
             AND length(auth_hostname) > 0
             AND length(auth_account) > 0
             AND auth_credential_id IS NULL AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'secret_service_bearer'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'secret_service_header'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND oauth_registration_json IS NULL
             AND length(auth_header_name) > 0
             AND auth_header_prefix IS NOT NULL)
            OR
            (auth_kind = 'oauth'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND json_valid(oauth_registration_json)
             AND json_type(oauth_registration_json) = 'object'
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_catalogs (
        connection_id TEXT PRIMARY KEY
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        ),
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

    CREATE TABLE mcp_oauth_flows (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'begin_in_flight', 'awaiting_callback', 'callback_ready',
                'exchange_in_flight', 'refresh_in_flight', 'unknown'
            )
        ),
        callback_port INTEGER,
        callback_relay_id TEXT,
        expires_at_ms INTEGER,
        CHECK (
            (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                       'exchange_in_flight')
             AND ((callback_port BETWEEN 1 AND 65535 AND callback_relay_id IS NULL)
                  OR (callback_port IS NULL AND length(callback_relay_id) = 36))
             AND expires_at_ms > 0)
            OR
            (phase IN ('refresh_in_flight', 'unknown')
             AND callback_port IS NULL
             AND callback_relay_id IS NULL
             AND expires_at_ms IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_oauth_receipts (
        connection_id TEXT NOT NULL CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        outcome_json TEXT NOT NULL CHECK (
            length(outcome_json) BETWEEN 1 AND 16384
            AND json_valid(outcome_json)
            AND json_type(outcome_json) = 'object'
        ),
        PRIMARY KEY (connection_id, operation_id)
    ) STRICT;

    CREATE TABLE skill_revisions (
        skill_digest TEXT PRIMARY KEY CHECK (length(skill_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 1024),
        license TEXT,
        compatibility TEXT,
        UNIQUE (skill_digest, name)
    ) STRICT;

    CREATE TABLE installed_plugins (
        plugin_digest TEXT PRIMARY KEY CHECK (length(plugin_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        version TEXT,
        description TEXT,
        homepage TEXT,
        repository TEXT,
        license TEXT
    ) STRICT;

    CREATE TABLE plugin_mcp_servers (
        plugin_digest TEXT NOT NULL
            REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
        server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
        transport TEXT NOT NULL CHECK (transport = 'streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        ),
        PRIMARY KEY (plugin_digest, server_id)
    ) STRICT;

    CREATE TABLE shared_plugin_registry_state (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        registry_id TEXT NOT NULL CHECK (length(registry_id) = 36),
        applied_revision INTEGER NOT NULL CHECK (applied_revision >= 0)
    ) STRICT;

    INSERT INTO host_metadata(singleton, schema_version) VALUES (1, 13);
";

pub(crate) fn initialize(path: &Path) -> Result<(), HostCatalogError> {
    let mut connection = open(path, rusqlite::OpenFlags::default())?;
    restrict_database_permissions(path)?;
    initialize_connection(&mut connection)
}

pub(crate) fn open_verified(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = open(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    verify(&connection)?;
    Ok(connection)
}

pub(crate) fn open_read_only(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    verify(&connection)?;
    Ok(connection)
}

fn open(path: &Path, flags: rusqlite::OpenFlags) -> Result<Connection, HostCatalogError> {
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )?;
    Ok(connection)
}

fn initialize_connection(connection: &mut Connection) -> Result<(), HostCatalogError> {
    let observed =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    if (1..SCHEMA_VERSION).contains(&observed) {
        return Err(HostCatalogError::Invalid(format!(
            "this data root holds schema {observed}; the canonical agent cutover discards agent-owned state, so start it with `renoa-host <config.json> reset <backup-directory>`"
        )));
    }
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
            agents::initialize(&transaction)?;
            crate::skills::SkillStore::initialize_tables(&transaction)?;
            super::definition::schema::initialize(&transaction)?;
            super::routines::initialize(&transaction)?;
            super::reviews::initialize(&transaction)?;
            transaction.execute(
                "UPDATE host_metadata SET schema_version=?1 WHERE singleton=1",
                [SCHEMA_VERSION],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            verify(connection)
        }
        found => Err(HostCatalogError::Invalid(format!(
            "schema {found} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

fn verify(connection: &Connection) -> Result<(), HostCatalogError> {
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
        return Err(HostCatalogError::Invalid(
            "metadata is missing or incompatible".to_owned(),
        ));
    }
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(2)?))
        })
        .optional()?;
    if let Some((table, parent)) = violation {
        return Err(HostCatalogError::Invalid(format!(
            "foreign-key validation failed: {table} references {parent}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_database_permissions(path: &Path) -> Result<(), HostCatalogError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_path: &Path) -> Result<(), HostCatalogError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HostCatalogError, cutover, initialize, open_verified};

    #[test]
    fn schema_ten_adds_an_unbound_shared_registry_cursor() {
        let directory = tempfile::tempdir().expect("temporary Host catalog");
        let database = directory.path().join("host.sqlite3");
        initialize(&database).expect("initialize current catalog");
        {
            let connection = open_verified(&database).expect("open current catalog");
            connection
                .execute_batch(
                    "INSERT INTO mcp_integrations(
                        integration_id, kind, endpoint, request_headers_json
                     ) VALUES (
                        'retained', 'direct_streamable_http', 'https://example.com/mcp', '{}'
                     );
                     DROP TABLE host_agents;
                     DROP TABLE host_identity;
                     DROP TABLE shared_plugin_registry_state;
                     UPDATE host_metadata SET schema_version = 10 WHERE singleton = 1;
                     PRAGMA user_version = 10;",
                )
                .expect("construct schema-ten fixture");
        }
        let refused = initialize(&database);
        assert!(
            matches!(&refused, Err(HostCatalogError::Invalid(message)) if message.contains("reset")),
            "an earlier data root must be refused until it is reset: {refused:?}"
        );
        cutover(&database).expect("cut over schema ten");
        initialize(&database).expect("open after the cutover");
        let connection = open_verified(&database).expect("open cut-over catalog");
        let rows = connection
            .query_row(
                "SELECT COUNT(*) FROM shared_plugin_registry_state",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("read shared registry state");
        assert_eq!(rows, 0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM mcp_integrations WHERE integration_id = 'retained'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("retained integration"),
            1
        );
    }

    #[test]
    fn a_creation_receipt_without_a_result_is_refused_and_repaired() {
        let directory = tempfile::tempdir().expect("temporary Host catalog");
        let database = directory.path().join("host.sqlite3");
        initialize(&database).expect("initialize current catalog");
        {
            let connection = open_verified(&database).expect("open current catalog");
            connection
                .execute_batch(
                    "DROP TABLE host_agent_creations;
                     CREATE TABLE host_agent_creations (
                        operation_id TEXT PRIMARY KEY,
                        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                        request_json TEXT NOT NULL CHECK (json_valid(request_json))
                     ) STRICT;
                     UPDATE host_metadata SET schema_version = 27 WHERE singleton = 1;
                     PRAGMA user_version = 27;",
                )
                .expect("construct schema-twenty-seven fixture");
        }
        let refused = initialize(&database);
        assert!(
            matches!(&refused, Err(HostCatalogError::Invalid(message)) if message.contains("reset")),
            "a receipt without a stored result must be refused until it is reset: {refused:?}"
        );
        cutover(&database).expect("cut over schema twenty-seven");
        initialize(&database).expect("open after the cutover");
        let connection = open_verified(&database).expect("open cut-over catalog");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('host_agent_creations')
                     WHERE name = 'result_json'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("read creation receipt columns"),
            1,
            "the cutover must restore the canonical receipt shape"
        );
    }
}
