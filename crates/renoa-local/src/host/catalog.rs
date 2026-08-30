use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};
use thiserror::Error;

mod migrations;

use migrations::{
    MIGRATE_V1_TO_V2, MIGRATE_V2_TO_V3, MIGRATE_V3_TO_V4, MIGRATE_V4_TO_V5, MIGRATE_V5_TO_V6,
    MIGRATE_V6_TO_V7, MIGRATE_V7_TO_V8, MIGRATE_V8_TO_V9, MIGRATE_V9_TO_V10, MIGRATE_V10_TO_V11,
};

const SCHEMA_VERSION: u32 = 11;
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

    CREATE TABLE profile_mcp_connections (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        connection_id TEXT NOT NULL
            REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
        PRIMARY KEY (profile_id, connection_id)
    ) STRICT;

    CREATE TABLE mcp_oauth_flows (
        connection_id TEXT PRIMARY KEY
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'begin_in_flight', 'awaiting_callback', 'callback_ready',
                'exchange_in_flight', 'refresh_in_flight', 'unknown'
            )
        ),
        callback_port INTEGER,
        expires_at_ms INTEGER,
        CHECK (
            (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                       'exchange_in_flight')
             AND callback_port BETWEEN 1 AND 65535
             AND expires_at_ms > 0)
            OR
            (phase IN ('refresh_in_flight', 'unknown')
             AND callback_port IS NULL
             AND expires_at_ms IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_oauth_receipts (
        connection_id TEXT NOT NULL
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
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

    CREATE TABLE profile_skill_bindings (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (
            scope_kind IN ('global', 'workspace', 'plugin')
        ),
        workspace TEXT,
        source_id TEXT NOT NULL CHECK (length(source_id) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        CHECK (
            (scope_kind IN ('global', 'plugin') AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_id, skill_name)
    ) STRICT;

    CREATE TABLE skill_source_rejections (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (
            scope_kind IN ('global', 'workspace', 'plugin')
        ),
        workspace TEXT,
        source_id TEXT NOT NULL CHECK (length(source_id) > 0),
        entry_name TEXT NOT NULL CHECK (length(entry_name) > 0),
        reason TEXT NOT NULL CHECK (length(reason) > 0),
        CHECK (
            (scope_kind IN ('global', 'plugin') AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_id, entry_name)
    ) STRICT;

    CREATE TABLE session_skills (
        activation_order INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL CHECK (length(session_id) > 0),
        activation_command_id TEXT NOT NULL CHECK (length(activation_command_id) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        UNIQUE (session_id, skill_name),
        UNIQUE (session_id, skill_digest)
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

    INSERT INTO host_metadata(singleton, schema_version) VALUES (1, 11);
";

pub(crate) fn initialize(path: &Path) -> Result<(), HostCatalogError> {
    let mut connection = open(path)?;
    restrict_database_permissions(path)?;
    initialize_connection(&mut connection)
}

pub(crate) fn open_verified(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = open(path)?;
    verify(&connection)?;
    Ok(connection)
}

fn open(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = Connection::open(path)?;
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
    if matches!(observed, 1..=10) {
        return migrate(connection);
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
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            verify(connection)
        }
        found => Err(HostCatalogError::Invalid(format!(
            "schema {found} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

fn migrate(connection: &mut Connection) -> Result<(), HostCatalogError> {
    connection.pragma_update(None, "foreign_keys", false)?;
    let migration = (|| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version =
            transaction.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
        match version {
            SCHEMA_VERSION => transaction.commit().map_err(HostCatalogError::from),
            version if (1..SCHEMA_VERSION).contains(&version) => {
                if version <= 2 {
                    require_complete_selected_catalogs(&transaction)?;
                }
                for (source_version, migration) in [
                    (1, MIGRATE_V1_TO_V2),
                    (2, MIGRATE_V2_TO_V3),
                    (3, MIGRATE_V3_TO_V4),
                    (4, MIGRATE_V4_TO_V5),
                    (5, MIGRATE_V5_TO_V6),
                    (6, MIGRATE_V6_TO_V7),
                    (7, MIGRATE_V7_TO_V8),
                    (8, MIGRATE_V8_TO_V9),
                    (9, MIGRATE_V9_TO_V10),
                    (10, MIGRATE_V10_TO_V11),
                ] {
                    if source_version >= version {
                        transaction.execute_batch(migration)?;
                    }
                }
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                transaction.commit()?;
                Ok(())
            }
            found => Err(HostCatalogError::Invalid(format!(
                "schema {found} is unsupported; expected {SCHEMA_VERSION}"
            ))),
        }
    })();
    let foreign_keys = connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(HostCatalogError::from);
    migration?;
    foreign_keys?;
    verify(connection)
}

fn require_complete_selected_catalogs(connection: &Connection) -> Result<(), HostCatalogError> {
    let missing = connection
        .query_row(
            "SELECT binding.connection_id
             FROM profile_mcp_tools AS binding
             LEFT JOIN mcp_catalogs AS catalog
               ON catalog.connection_id = binding.connection_id
             WHERE catalog.connection_id IS NULL
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(connection_id) = missing {
        return Err(HostCatalogError::Invalid(format!(
            "selected connection '{connection_id}' has no complete catalog"
        )));
    }
    Ok(())
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
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?;
    if violation.is_some() {
        return Err(HostCatalogError::Invalid(
            "foreign-key validation failed".to_owned(),
        ));
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
    use super::{initialize, open_verified};

    #[test]
    fn schema_ten_adds_an_unbound_shared_registry_cursor() {
        let directory = tempfile::tempdir().expect("temporary Host catalog");
        let database = directory.path().join("host.sqlite3");
        initialize(&database).expect("initialize current catalog");
        {
            let connection = open_verified(&database).expect("open current catalog");
            connection
                .execute_batch(
                    "DROP TABLE shared_plugin_registry_state;
                     UPDATE host_metadata SET schema_version = 10 WHERE singleton = 1;
                     PRAGMA user_version = 10;",
                )
                .expect("construct schema-ten fixture");
        }
        initialize(&database).expect("migrate schema ten");
        let connection = open_verified(&database).expect("open migrated catalog");
        let rows = connection
            .query_row(
                "SELECT COUNT(*) FROM shared_plugin_registry_state",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("read shared registry state");
        assert_eq!(rows, 0);
    }
}
