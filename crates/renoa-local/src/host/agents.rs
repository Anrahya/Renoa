use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog};
use crate::{AgentProfileId, host_storage::SessionManifest};

mod inventory;

/// A durable agent, independent of its sessions and the process running them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub profile: AgentProfileId,
    pub name: String,
    pub created_by: Option<AgentId>,
}

impl LocalHost {
    /// Returns the durable identity of this Host data root.
    ///
    /// # Errors
    /// Returns catalog storage or identity corruption errors.
    pub async fn host_id(&self) -> Result<Uuid, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            let value: String = connection
                .query_row(
                    "SELECT host_id FROM host_identity WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(catalog::HostCatalogError::from)?;
            parse_uuid(&value)
        })
        .await?
    }

    /// Creates one named agent, or returns the identical existing record.
    ///
    /// The caller supplies a stable identity before sending this operation.
    /// Reusing it with different fields conflicts. A creator must already exist;
    /// creation therefore cannot introduce a cycle or a dangling parent.
    ///
    /// # Errors
    /// Returns invalid profile/name, identity conflict, missing creator, or storage errors.
    pub async fn ensure_agent(&self, record: AgentRecord) -> Result<AgentRecord, LocalHostError> {
        self.profile(&record.profile).await?;
        validate(&record)?;
        let database = self.config.database.clone();
        let sessions = self.config.sessions.clone();
        tokio::task::spawn_blocking(move || {
            import_binding(&database, &sessions, record.id)?;
            if let Some(parent) = record.created_by {
                import_binding(&database, &sessions, parent)?;
            }
            ensure(&database, record, false)
        })
        .await?
    }

    /// Looks up one agent without discovering a model or constructing a runtime.
    ///
    /// # Errors
    /// Returns catalog storage or record corruption errors.
    pub async fn agent(&self, id: AgentId) -> Result<Option<AgentRecord>, LocalHostError> {
        let database = self.config.database.clone();
        let sessions = self.config.sessions.clone();
        tokio::task::spawn_blocking(move || {
            import_binding(&database, &sessions, id)?;
            let connection = catalog::open_verified(&database)?;
            read(&connection, id)
        })
        .await?
    }

    /// Lists durable agents, importing bindings of sessions created before the catalog.
    ///
    /// No model or diagnostic store is required. Invalid published manifests are
    /// reported explicitly. Deleted sessions do not delete their owning agents.
    ///
    /// # Errors
    /// Returns manifest, identity conflict, or catalog storage errors.
    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>, LocalHostError> {
        let database = self.config.database.clone();
        let sessions = self.config.sessions.clone();
        tokio::task::spawn_blocking(move || {
            for manifest in inventory::manifests(&sessions)? {
                ensure_manifest(&database, &manifest)?;
            }
            let connection = catalog::open_verified(&database)?;
            let mut query = connection.prepare(
                "SELECT agent_id, profile_id, name, created_by FROM host_agents ORDER BY agent_id",
            ).map_err(catalog::HostCatalogError::from)?;
            let rows = query
                .query_map([], decode_row)
                .map_err(catalog::HostCatalogError::from)?;
            rows.map(|row| decode(row.map_err(catalog::HostCatalogError::from)?))
                .collect()
        })
        .await?
    }

    pub(super) async fn retain_agent_binding(
        &self,
        manifest: &SessionManifest,
    ) -> Result<(), LocalHostError> {
        let database = self.config.database.clone();
        let record = legacy_record(manifest);
        tokio::task::spawn_blocking(move || ensure(&database, record, true)).await??;
        Ok(())
    }
}

fn legacy_record(manifest: &SessionManifest) -> AgentRecord {
    AgentRecord {
        id: manifest.agent_id,
        profile: manifest.profile.clone(),
        name: manifest.profile.to_string(),
        created_by: None,
    }
}

// A pre-catalog session must not lose its identity to a new creation using
// that ID. Published manifests also repair an interrupted catalog write.
fn import_binding(database: &Path, sessions: &Path, id: AgentId) -> Result<(), LocalHostError> {
    for manifest in inventory::manifests(sessions)? {
        if manifest.agent_id == id {
            ensure_manifest(database, &manifest)?;
        }
    }
    Ok(())
}

pub(super) fn ensure_manifest(
    database: &Path,
    manifest: &SessionManifest,
) -> Result<(), LocalHostError> {
    ensure(database, legacy_record(manifest), true)?;
    Ok(())
}

fn ensure(
    database: &Path,
    record: AgentRecord,
    binding_only: bool,
) -> Result<AgentRecord, LocalHostError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(catalog::HostCatalogError::from)?;
    if let Some(existing) = read(&transaction, record.id)? {
        if existing.profile != record.profile || (!binding_only && existing != record) {
            return Err(LocalHostError::AgentConflict(record.id));
        }
        transaction
            .commit()
            .map_err(catalog::HostCatalogError::from)?;
        return Ok(existing);
    }
    validate(&record)?;
    if let Some(parent) = record.created_by
        && read(&transaction, parent)?.is_none()
    {
        return Err(LocalHostError::AgentNotFound(parent));
    }
    transaction.execute(
        "INSERT INTO host_agents(agent_id, profile_id, name, created_by) VALUES (?1, ?2, ?3, ?4)",
        params![record.id.to_string(), record.profile.as_str(), record.name, record.created_by.map(|id| id.to_string())],
    ).map_err(catalog::HostCatalogError::from)?;
    transaction
        .commit()
        .map_err(catalog::HostCatalogError::from)?;
    Ok(record)
}

fn validate(record: &AgentRecord) -> Result<(), LocalHostError> {
    if record.name.trim().is_empty()
        || record.name.len() > 512
        || record.created_by == Some(record.id)
    {
        return Err(LocalHostError::InvalidRequest("agent name must contain 1–512 bytes of nonblank text and its creator must be another agent".to_owned()));
    }
    Ok(())
}

type StoredRecord = (String, String, String, Option<String>);

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRecord> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode((id, profile, name, parent): StoredRecord) -> Result<AgentRecord, LocalHostError> {
    let record = AgentRecord {
        id: AgentId::from_uuid(parse_uuid(&id)?),
        profile: AgentProfileId::new(profile)?,
        name,
        created_by: parent
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(AgentId::from_uuid),
    };
    validate(&record)?;
    Ok(record)
}

fn parse_uuid(value: &str) -> Result<Uuid, LocalHostError> {
    Uuid::parse_str(value).map_err(|error| {
        catalog::HostCatalogError::Invalid(format!("invalid stored identity: {error}")).into()
    })
}

fn read(connection: &Connection, id: AgentId) -> Result<Option<AgentRecord>, LocalHostError> {
    connection
        .query_row(
            "SELECT agent_id, profile_id, name, created_by FROM host_agents WHERE agent_id = ?1",
            [id.to_string()],
            decode_row,
        )
        .optional()
        .map_err(catalog::HostCatalogError::from)?
        .map(decode)
        .transpose()
}
