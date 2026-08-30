use std::{path::Path, time::Duration};

use renoa_registry_protocol::RegistryId;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use crate::store::RegistryError;

const SCHEMA_VERSION: u32 = 1;
const SCHEMA: &str = "
    CREATE TABLE registry_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        schema_version INTEGER NOT NULL CHECK (schema_version = 1),
        registry_id TEXT NOT NULL CHECK (length(registry_id) = 36)
    ) STRICT;

    CREATE TABLE packages (
        package_digest TEXT PRIMARY KEY CHECK (length(package_digest) = 64),
        archive_digest TEXT NOT NULL CHECK (length(archive_digest) = 64),
        archive_bytes INTEGER NOT NULL CHECK (archive_bytes > 0),
        revision INTEGER NOT NULL UNIQUE CHECK (revision > 0)
    ) STRICT;
";

pub(crate) fn initialize(path: &Path) -> Result<RegistryId, RegistryError> {
    let mut connection = open(path)?;
    restrict_database_permissions(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version =
        transaction.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    match version {
        0 => {
            let registry_id = RegistryId::new();
            transaction.execute_batch(SCHEMA)?;
            transaction.execute(
                "INSERT INTO registry_metadata(singleton, schema_version, registry_id)
                 VALUES (1, ?1, ?2)",
                params![SCHEMA_VERSION, registry_id.to_string()],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            transaction.commit()?;
            Ok(registry_id)
        }
        SCHEMA_VERSION => {
            let registry_id = read_identity(&transaction)?;
            transaction.commit()?;
            verify(&connection, registry_id)?;
            Ok(registry_id)
        }
        found => Err(RegistryError::InvalidState(format!(
            "registry schema {found} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

pub(crate) fn open_verified(path: &Path) -> Result<Connection, RegistryError> {
    let connection = open(path)?;
    let registry_id = read_identity(&connection)?;
    verify(&connection, registry_id)?;
    Ok(connection)
}

fn open(path: &Path) -> Result<Connection, RegistryError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )?;
    Ok(connection)
}

fn read_identity(connection: &Connection) -> Result<RegistryId, RegistryError> {
    let identity = connection
        .query_row(
            "SELECT registry_id FROM registry_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| RegistryError::InvalidState("registry metadata is missing".to_owned()))?;
    identity.parse().map_err(|error| {
        RegistryError::InvalidState(format!("registry identity is invalid: {error}"))
    })
}

fn verify(connection: &Connection, expected_id: RegistryId) -> Result<(), RegistryError> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    if version != SCHEMA_VERSION || read_identity(connection)? != expected_id {
        return Err(RegistryError::InvalidState(
            "registry metadata is missing or incompatible".to_owned(),
        ));
    }
    let integrity =
        connection.pragma_query_value(None, "quick_check", |row| row.get::<_, String>(0))?;
    if integrity != "ok" {
        return Err(RegistryError::InvalidState(format!(
            "registry database integrity check failed: {integrity}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_database_permissions(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_path: &Path) -> Result<(), RegistryError> {
    Ok(())
}
