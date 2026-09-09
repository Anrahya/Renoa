use rusqlite::Connection;
use serde::Serialize;
use uuid::Uuid;

use super::{HostCatalogError, parse_id};
use crate::{AgentProfileId, RoutineSchedule};

#[derive(Debug, Serialize)]
pub struct ObservedAgent {
    pub id: Uuid,
    pub profile: String,
    pub name: String,
    pub created_by: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ObservedRoutine {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub schedule: RoutineSchedule,
    pub enabled: bool,
    pub revision: i64,
    pub next_due_ms: i64,
    pub pending_runs: u64,
    pub completed_runs: u64,
}

#[derive(Debug, Serialize)]
pub struct ObservedConnection {
    pub id: String,
    /// A stored catalog is not evidence of a currently healthy remote connection.
    pub catalog_available: bool,
    pub tool_count: u64,
    pub selected_by_profiles: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ObservedPlugin {
    pub digest: String,
    pub name: String,
    pub version: Option<String>,
}

/// A recorded revision, not a claim that a session has loaded its instructions.
#[derive(Debug, Serialize)]
pub struct ObservedSkill {
    pub digest: String,
    pub name: String,
}

pub(super) fn agents(db: &Connection) -> Result<Vec<ObservedAgent>, HostCatalogError> {
    let mut q = db
        .prepare("SELECT agent_id,profile_id,name,created_by FROM host_agents ORDER BY agent_id")?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let profile: String = row.get(1)?;
        AgentProfileId::new(&profile).map_err(|e| HostCatalogError::Invalid(e.to_string()))?;
        items.push(ObservedAgent {
            id: parse_id(&row.get::<_, String>(0)?)?,
            profile,
            name: row.get(2)?,
            created_by: row
                .get::<_, Option<String>>(3)?
                .as_deref()
                .map(parse_id)
                .transpose()?,
        });
    }
    Ok(items)
}

pub(super) fn routines(db: &Connection) -> Result<Vec<ObservedRoutine>, HostCatalogError> {
    let mut q = db.prepare("SELECT r.id,r.agent_id,r.name,r.schedule_json,r.enabled,r.revision,r.next_due_ms,
        (SELECT count(*) FROM host_routine_runs x WHERE x.routine_id=r.id AND x.output IS NULL),
        (SELECT count(*) FROM host_routine_runs x WHERE x.routine_id=r.id AND x.output IS NOT NULL)
        FROM host_routines r WHERE NOT EXISTS (SELECT 1 FROM host_routine_deletions d WHERE d.routine_id=r.id)
        ORDER BY r.id")?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(ObservedRoutine {
            id: parse_id(&row.get::<_, String>(0)?)?,
            agent_id: parse_id(&row.get::<_, String>(1)?)?,
            name: row.get(2)?,
            schedule: serde_json::from_str(&row.get::<_, String>(3)?)
                .map_err(|e| HostCatalogError::Invalid(format!("invalid schedule: {e}")))?,
            enabled: row.get(4)?,
            revision: row.get(5)?,
            next_due_ms: row.get(6)?,
            pending_runs: count(row, 7)?,
            completed_runs: count(row, 8)?,
        });
    }
    Ok(items)
}

pub(super) fn connections(db: &Connection) -> Result<Vec<ObservedConnection>, HostCatalogError> {
    // Never select endpoint URLs, headers, OAuth state or credential references.
    let mut q = db.prepare("SELECT c.connection_id, EXISTS(SELECT 1 FROM mcp_catalogs m WHERE m.connection_id=c.connection_id),
        (SELECT count(*) FROM mcp_tools t WHERE t.connection_id=c.connection_id)
        FROM mcp_connections c ORDER BY c.connection_id")?;
    let mut profiles = db.prepare(
        "SELECT profile_id FROM profile_mcp_connections WHERE connection_id=?1 ORDER BY profile_id",
    )?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let selected_by_profiles = profiles
            .query_map([&id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        items.push(ObservedConnection {
            id,
            catalog_available: row.get(1)?,
            tool_count: count(row, 2)?,
            selected_by_profiles,
        });
    }
    Ok(items)
}

pub(super) fn plugins(db: &Connection) -> Result<Vec<ObservedPlugin>, HostCatalogError> {
    let mut q = db.prepare(
        "SELECT plugin_digest,name,version FROM installed_plugins ORDER BY plugin_digest",
    )?;
    Ok(q.query_map([], |r| {
        Ok(ObservedPlugin {
            digest: r.get(0)?,
            name: r.get(1)?,
            version: r.get(2)?,
        })
    })?
    .collect::<Result<_, _>>()?)
}

pub(super) fn skills(db: &Connection) -> Result<Vec<ObservedSkill>, HostCatalogError> {
    let mut q =
        db.prepare("SELECT skill_digest,name FROM skill_revisions ORDER BY name,skill_digest")?;
    Ok(q.query_map([], |r| {
        Ok(ObservedSkill {
            digest: r.get(0)?,
            name: r.get(1)?,
        })
    })?
    .collect::<Result<_, _>>()?)
}

fn count(row: &rusqlite::Row<'_>, index: usize) -> Result<u64, HostCatalogError> {
    u64::try_from(row.get::<_, i64>(index)?)
        .map_err(|e| HostCatalogError::Invalid(format!("invalid inventory count: {e}")))
}
