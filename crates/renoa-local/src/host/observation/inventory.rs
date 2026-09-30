use rusqlite::{Connection, OptionalExtension as _};
use serde::Serialize;
use uuid::Uuid;

use super::{HostCatalogError, parse_id};
use crate::{AutomationSchedule, RunStatus};

#[derive(Debug, Serialize)]
pub struct ObservedAgent {
    pub id: Uuid,
    pub name: String,
    /// The creator agent id when an agent created this agent, otherwise null.
    pub created_by: Option<Uuid>,
    pub preset_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ObservedAutomation {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub schedule: AutomationSchedule,
    pub enabled: bool,
    pub revision: i64,
    pub next_due_ms: i64,
    pub pending_runs: u64,
    /// Every finished run, whatever its status.
    pub completed_runs: u64,
    pub failed_runs: u64,
    pub skipped_runs: u64,
    /// The most recently finished run.
    pub last_run: Option<ObservedRun>,
}

#[derive(Debug, Serialize)]
pub struct ObservedRun {
    pub id: Uuid,
    pub status: RunStatus,
    pub due_ms: i64,
    /// `None` for runs recorded before schema 37.
    pub finished_at_ms: Option<i64>,
    /// `None` for a skipped run and for runs recorded before schema 37.
    pub failed_tool_calls: Option<u32>,
}

/// The process owning the automation schedule records a heartbeat
/// periodically while it runs, so an old one means no scheduler is running.
/// The executor sets the period (`renoa-node`: 30 seconds).
#[derive(Debug, Serialize)]
pub struct ObservedScheduler {
    pub heartbeat_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct ObservedConnection {
    pub id: String,
    /// A stored catalog is not evidence of a currently healthy remote connection.
    pub catalog_available: bool,
    pub tool_count: u64,
    pub selected_by_agents: Vec<String>,
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

/// This Host's binding to a shared plugin registry.
#[derive(Debug, Serialize)]
pub struct ObservedSharedRegistry {
    /// The registry this Host synchronizes with; null until the first
    /// synchronization binds one.
    pub registry_id: Option<Uuid>,
    pub applied_revision: u64,
    /// Present while the latest synchronization failed.
    pub failure: Option<ObservedRegistryFailure>,
}

#[derive(Debug, Serialize)]
pub struct ObservedRegistryFailure {
    /// When the current run of failures began.
    pub since_ms: i64,
    /// The latest failure's reason.
    pub error: String,
}

pub(super) fn agents(db: &Connection) -> Result<Vec<ObservedAgent>, HostCatalogError> {
    let mut q = db.prepare(
        "SELECT agent_id,name,preset_id,creator_agent_id
         FROM host_agents ORDER BY agent_id",
    )?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(ObservedAgent {
            id: parse_id(&row.get::<_, String>(0)?)?,
            name: row.get(1)?,
            preset_id: row.get(2)?,
            created_by: row
                .get::<_, Option<String>>(3)?
                .as_deref()
                .map(parse_id)
                .transpose()?,
        });
    }
    Ok(items)
}

pub(super) fn automations(db: &Connection) -> Result<Vec<ObservedAutomation>, HostCatalogError> {
    let mut q = db.prepare("SELECT r.id,r.agent_id,r.name,r.schedule_json,r.enabled,r.revision,r.next_due_ms,
        (SELECT count(*) FROM host_automation_runs x WHERE x.automation_id=r.id AND x.output IS NULL),
        (SELECT count(*) FROM host_automation_runs x WHERE x.automation_id=r.id AND x.output IS NOT NULL),
        (SELECT count(*) FROM host_automation_runs x WHERE x.automation_id=r.id AND x.status='failed'),
        (SELECT count(*) FROM host_automation_runs x WHERE x.automation_id=r.id AND x.status='skipped')
        FROM host_automations r WHERE NOT EXISTS (SELECT 1 FROM host_automation_deletions d WHERE d.automation_id=r.id)
        ORDER BY r.id")?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(ObservedAutomation {
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
            failed_runs: count(row, 9)?,
            skipped_runs: count(row, 10)?,
            last_run: None,
        });
    }
    for automation in &mut items {
        automation.last_run = last_run(db, automation.id)?;
    }
    Ok(items)
}

fn last_run(db: &Connection, automation: Uuid) -> Result<Option<ObservedRun>, HostCatalogError> {
    db.query_row(
        "SELECT id,status,due_ms,finished_at_ms,failed_tool_calls FROM host_automation_runs
         WHERE automation_id=?1 AND output IS NOT NULL ORDER BY sequence DESC LIMIT 1",
        [automation.to_string()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )
    .optional()?
    .map(|(id, status, due_ms, finished_at_ms, failed_tool_calls)| {
        Ok(ObservedRun {
            id: parse_id(&id)?,
            status,
            due_ms,
            finished_at_ms,
            failed_tool_calls,
        })
    })
    .transpose()
}

pub(super) fn scheduler(db: &Connection) -> Result<Option<ObservedScheduler>, HostCatalogError> {
    Ok(db
        .query_row(
            "SELECT heartbeat_ms FROM host_automation_scheduler WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .map(|heartbeat_ms| ObservedScheduler { heartbeat_ms }))
}

pub(super) fn connections(db: &Connection) -> Result<Vec<ObservedConnection>, HostCatalogError> {
    // Never select endpoint URLs, headers, OAuth state or credential references.
    let mut q = db.prepare("SELECT c.connection_id, EXISTS(SELECT 1 FROM mcp_catalogs m WHERE m.connection_id=c.connection_id),
        (SELECT count(*) FROM mcp_tools t WHERE t.connection_id=c.connection_id)
        FROM mcp_connections c ORDER BY c.connection_id")?;
    let mut agents = db.prepare(
        "SELECT agent_id FROM host_agent_mcp_connections WHERE connection_id=?1 ORDER BY agent_id",
    )?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let selected_by_agents = agents
            .query_map([&id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        items.push(ObservedConnection {
            id,
            catalog_available: row.get(1)?,
            tool_count: count(row, 2)?,
            selected_by_agents,
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

/// Null when this Host has neither bound a shared registry nor failed to
/// synchronize with one.
pub(super) fn shared_registry(
    db: &Connection,
) -> Result<Option<ObservedSharedRegistry>, HostCatalogError> {
    let binding = db
        .query_row(
            "SELECT registry_id, applied_revision FROM shared_plugin_registry_state
             WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let failure = db
        .query_row(
            "SELECT failing_since_ms, error FROM shared_plugin_registry_sync
             WHERE singleton = 1",
            [],
            |row| {
                Ok(ObservedRegistryFailure {
                    since_ms: row.get(0)?,
                    error: row.get(1)?,
                })
            },
        )
        .optional()?;
    if binding.is_none() && failure.is_none() {
        return Ok(None);
    }
    let (registry_id, applied_revision) = match binding {
        Some((id, revision)) => (
            Some(parse_id(&id)?),
            u64::try_from(revision).map_err(|_| {
                HostCatalogError::Invalid("stored shared registry revision is negative".to_owned())
            })?,
        ),
        None => (None, 0),
    };
    Ok(Some(ObservedSharedRegistry {
        registry_id,
        applied_revision,
        failure,
    }))
}
