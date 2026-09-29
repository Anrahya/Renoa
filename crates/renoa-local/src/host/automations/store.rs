use super::{
    AutomationError, AutomationMutation, AutomationRecord, AutomationSchedule, AutomationSpec,
    receipts::AutomationActor,
};
use crate::host::catalog;
use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(in crate::host) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS host_automations (
        id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        name TEXT NOT NULL, prompt TEXT NOT NULL, schedule_json TEXT NOT NULL CHECK(json_valid(schedule_json)),
        enabled INTEGER NOT NULL CHECK(enabled IN(0,1)), revision INTEGER NOT NULL CHECK(revision>0), next_due_ms INTEGER NOT NULL,
        origin_session_id TEXT
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_automation_deletions (automation_id TEXT PRIMARY KEY REFERENCES host_automations(id)) STRICT;
    CREATE TABLE IF NOT EXISTS host_automation_mutations (
        operation_id TEXT PRIMARY KEY, actor_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        request_json TEXT NOT NULL, result_json TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_automation_runs (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
        automation_id TEXT NOT NULL REFERENCES host_automations(id), agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        due_ms INTEGER NOT NULL, admitted_at_ms INTEGER NOT NULL, prompt TEXT NOT NULL, output TEXT,
        status TEXT CHECK(status IN ('succeeded','failed','skipped')),
        failed_tool_calls INTEGER CHECK(failed_tool_calls >= 0), finished_at_ms INTEGER
    ) STRICT;
    CREATE INDEX IF NOT EXISTS host_automation_pending ON host_automation_runs(sequence) WHERE output IS NULL;
    UPDATE host_metadata SET schema_version=16 WHERE singleton=1;")?;
    tx.execute_batch(super::runs::SCHEDULER_TABLE)?;
    super::receipts::initialize(tx)
}

fn active(cancellation: &CancellationToken) -> Result<(), AutomationError> {
    if cancellation.is_cancelled() {
        Err(AutomationError::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn mutate(
    path: &Path,
    actor: AutomationActor,
    operation: Uuid,
    mutation: AutomationMutation,
    now_ms: i64,
    cancellation: &CancellationToken,
) -> Result<AutomationRecord, AutomationError> {
    active(cancellation)?;
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(cancellation)?;
    let request = serde_json::to_string(&mutation)?;
    if let Some(record) = actor.replay(&tx, operation, &request)? {
        return Ok(record);
    }
    let record = match mutation {
        AutomationMutation::Create { spec } => {
            spec.validate(now_ms)?;
            actor.authorize(&tx, spec.agent_id)?;
            let origin = actor.origin_for(spec.agent_id);
            let record = AutomationRecord {
                id: operation,
                revision: 1,
                next_due_ms: spec.schedule.first_due(now_ms, spec.enabled)?,
                spec,
            };
            save(&tx, &record, true)?;
            if let Some(session) = origin {
                tx.execute(
                    "UPDATE host_automations SET origin_session_id=?2 WHERE id=?1",
                    params![record.id.to_string(), session.to_string()],
                )?;
            }
            record
        }
        AutomationMutation::Update {
            id,
            expected_revision,
            spec,
        } => {
            let old = get(&tx, id)?;
            actor.authorize(&tx, old.spec.agent_id)?;
            let record = updated_record(&old, expected_revision, spec, now_ms)?;
            save(&tx, &record, false)?;
            record
        }
        AutomationMutation::SetEnabled {
            id,
            expected_revision,
            enabled,
        } => {
            let old = get(&tx, id)?;
            actor.authorize(&tx, old.spec.agent_id)?;
            let mut spec = old.spec.clone();
            spec.enabled = enabled;
            let record = updated_record(&old, expected_revision, spec, now_ms)?;
            save(&tx, &record, false)?;
            record
        }
        AutomationMutation::Delete {
            id,
            expected_revision,
        } => {
            let mut record = get(&tx, id)?;
            actor.authorize(&tx, record.spec.agent_id)?;
            if record.revision != expected_revision {
                return Err(AutomationError::Conflict);
            }
            record.spec.enabled = false;
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or(AutomationError::Conflict)?;
            save(&tx, &record, false)?;
            tx.execute(
                "INSERT INTO host_automation_deletions(automation_id) VALUES(?1)",
                [id.to_string()],
            )?;
            record
        }
        AutomationMutation::RunNow { id } => {
            let mut record = get(&tx, id)?;
            actor.authorize(&tx, record.spec.agent_id)?;
            if now_ms < 0 {
                return Err(AutomationError::Invalid(
                    "invalid occurrence time".to_owned(),
                ));
            }
            if pending_for(&tx, id)? {
                return Err(AutomationError::Busy);
            }
            super::runs::insert_run(&tx, &record, operation, now_ms, now_ms)?;
            disarm_once(&mut record)?;
            save(&tx, &record, false)?;
            record
        }
    };
    active(cancellation)?;
    actor.save(&tx, operation, &request, &record)?;
    tx.commit()?;
    Ok(record)
}

fn updated_record(
    old: &AutomationRecord,
    expected_revision: i64,
    spec: AutomationSpec,
    now_ms: i64,
) -> Result<AutomationRecord, AutomationError> {
    if old.revision != expected_revision || old.spec.agent_id != spec.agent_id {
        return Err(AutomationError::Conflict);
    }
    spec.validate(now_ms)?;
    let next_due_ms = if old.spec.schedule == spec.schedule && old.spec.enabled == spec.enabled {
        old.next_due_ms
    } else {
        spec.schedule.first_due(now_ms, spec.enabled)?
    };
    Ok(AutomationRecord {
        id: old.id,
        revision: old
            .revision
            .checked_add(1)
            .ok_or(AutomationError::Conflict)?,
        spec,
        next_due_ms,
    })
}

// Disarming and admitting share a transaction. Bump the revision so an edit
// based on the armed state cannot accidentally re-arm a consumed occurrence.
pub(super) fn disarm_once(record: &mut AutomationRecord) -> Result<bool, AutomationError> {
    if !matches!(record.spec.schedule, AutomationSchedule::Once { .. }) {
        return Ok(false);
    }
    if record.spec.enabled {
        record.spec.enabled = false;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(AutomationError::Conflict)?;
    }
    Ok(true)
}

pub(super) fn authorize(
    db: &Connection,
    actor: AgentId,
    target: AgentId,
) -> Result<(), AutomationError> {
    if actor == target {
        return Ok(());
    }
    let manages_agents = crate::plugins::host::state::enabled_in(
        db,
        actor,
        crate::plugins::host::HostPluginId::Agents,
    )?;
    if manages_agents {
        Ok(())
    } else {
        Err(AutomationError::Invalid(format!(
            "an agent needs the enabled `{}` plugin to manage another agent's automations",
            "renoa.agents"
        )))
    }
}

/// Inserts an automation inside a caller's existing transaction.
///
/// Agent creation uses this so the agent, its capability rows, its receipt, and
/// its first automation commit together or not at all. Authorization is inherent:
/// the caller is creating the agent the automation belongs to.
pub(in crate::host) fn insert_first_automation(
    transaction: &Transaction<'_>,
    id: Uuid,
    spec: AutomationSpec,
    now_ms: i64,
) -> Result<AutomationRecord, AutomationError> {
    spec.validate(now_ms)?;
    let record = AutomationRecord {
        id,
        revision: 1,
        next_due_ms: spec.schedule.first_due(now_ms, spec.enabled)?,
        spec,
    };
    save(transaction, &record, true)?;
    Ok(record)
}

pub(super) fn save(
    tx: &Transaction<'_>,
    r: &AutomationRecord,
    create: bool,
) -> Result<(), AutomationError> {
    let sql = if create {
        "INSERT INTO host_automations(id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
    } else {
        "UPDATE host_automations SET agent_id=?2,name=?3,prompt=?4,schedule_json=?5,enabled=?6,revision=?7,next_due_ms=?8 WHERE id=?1"
    };
    tx.execute(
        sql,
        params![
            r.id.to_string(),
            r.spec.agent_id.to_string(),
            r.spec.name,
            r.spec.prompt,
            serde_json::to_string(&r.spec.schedule)?,
            r.spec.enabled,
            r.revision,
            r.next_due_ms
        ],
    )?;
    Ok(())
}

pub(super) fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationRecord> {
    Ok(AutomationRecord {
        id: parse(row, 0)?,
        spec: AutomationSpec {
            agent_id: AgentId::from_uuid(parse(row, 1)?),
            name: row.get(2)?,
            prompt: row.get(3)?,
            schedule: json_column(row, 4)?,
            enabled: row.get(5)?,
        },
        revision: row.get(6)?,
        next_due_ms: row.get(7)?,
    })
}
pub(super) fn parse(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let value: String = row.get(index)?;
    Uuid::parse_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn json_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let value: String = row.get(index)?;
    serde_json::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
pub(super) fn get(db: &Connection, id: Uuid) -> Result<AutomationRecord, AutomationError> {
    db.query_row("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_automations WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id=host_automations.id)",[id.to_string()],record).optional()?.ok_or(AutomationError::NotFound)
}
pub(super) fn list(
    db: &Connection,
    agent: AgentId,
    after: Option<Uuid>,
) -> Result<Vec<AutomationRecord>, AutomationError> {
    let mut query=db.prepare("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_automations WHERE agent_id=?1 AND id>?2 AND NOT EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id=host_automations.id) ORDER BY id LIMIT 20")?;
    Ok(query
        .query_map(
            params![
                agent.to_string(),
                after.map_or_else(String::new, |id| id.to_string())
            ],
            record,
        )?
        .collect::<Result<Vec<_>, _>>()?)
}
fn pending_for(db: &Connection, id: Uuid) -> Result<bool, AutomationError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_automation_runs WHERE automation_id=?1 AND output IS NULL)",
        [id.to_string()],
        |row| row.get(0),
    )?)
}
/// The Host session an automation was created in, when its own agent created
/// it from one. Deleted automations keep theirs, so an admitted run still
/// returns to its conversation.
pub(super) fn origin(db: &Connection, automation: Uuid) -> Result<Option<Uuid>, AutomationError> {
    let value: Option<String> = db.query_row(
        "SELECT origin_session_id FROM host_automations WHERE id=?1",
        [automation.to_string()],
        |row| row.get(0),
    )?;
    value
        .map(|value| {
            Uuid::parse_str(&value).map_err(|error| {
                AutomationError::Invalid(format!("stored origin session: {error}"))
            })
        })
        .transpose()
}
