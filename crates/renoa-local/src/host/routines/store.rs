use super::{
    RoutineError, RoutineMutation, RoutineRecord, RoutineRun, RoutineSchedule, RoutineSpec,
};
use crate::host::catalog;
use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(in crate::host) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS host_routines (
        id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        name TEXT NOT NULL, prompt TEXT NOT NULL, schedule_json TEXT NOT NULL CHECK(json_valid(schedule_json)),
        enabled INTEGER NOT NULL CHECK(enabled IN(0,1)), revision INTEGER NOT NULL CHECK(revision>0), next_due_ms INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_routine_mutations (
        operation_id TEXT PRIMARY KEY, actor_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        request_json TEXT NOT NULL, result_json TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_routine_runs (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
        routine_id TEXT NOT NULL REFERENCES host_routines(id), agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        session_id TEXT NOT NULL, due_ms INTEGER NOT NULL, admitted_at_ms INTEGER NOT NULL, prompt TEXT NOT NULL, output TEXT
    ) STRICT;
    CREATE INDEX IF NOT EXISTS host_routine_pending ON host_routine_runs(sequence) WHERE output IS NULL;
    UPDATE host_metadata SET schema_version=16 WHERE singleton=1;")?;
    Ok(())
}

fn active(cancellation: &CancellationToken) -> Result<(), RoutineError> {
    if cancellation.is_cancelled() {
        Err(RoutineError::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn mutate(
    path: &Path,
    actor: AgentId,
    operation: Uuid,
    mutation: RoutineMutation,
    now_ms: i64,
    cancellation: &CancellationToken,
) -> Result<RoutineRecord, RoutineError> {
    active(cancellation)?;
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(cancellation)?;
    let request = serde_json::to_string(&mutation)?;
    let receipt: Option<(String,String,String)> = tx.query_row("SELECT actor_id,request_json,result_json FROM host_routine_mutations WHERE operation_id=?1",[operation.to_string()],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    if let Some((old_actor, old_request, result)) = receipt {
        if old_actor != actor.to_string() || old_request != request {
            return Err(RoutineError::Conflict);
        }
        return Ok(serde_json::from_str(&result)?);
    }
    let record = match mutation {
        RoutineMutation::Create { spec } => {
            spec.validate(now_ms)?;
            authorize(&tx, actor, spec.agent_id)?;
            let record = RoutineRecord {
                id: operation,
                revision: 1,
                next_due_ms: spec.schedule.first_due(now_ms, spec.enabled)?,
                spec,
            };
            save(&tx, &record, true)?;
            record
        }
        RoutineMutation::Update {
            id,
            expected_revision,
            spec,
        } => {
            spec.validate(now_ms)?;
            let old = get(&tx, id)?;
            authorize(&tx, actor, old.spec.agent_id)?;
            if old.revision != expected_revision || old.spec.agent_id != spec.agent_id {
                return Err(RoutineError::Conflict);
            }
            let next_due_ms =
                if old.spec.schedule == spec.schedule && old.spec.enabled == spec.enabled {
                    old.next_due_ms
                } else {
                    spec.schedule.first_due(now_ms, spec.enabled)?
                };
            let revision = old.revision.checked_add(1).ok_or(RoutineError::Conflict)?;
            let record = RoutineRecord {
                id,
                revision,
                spec,
                next_due_ms,
            };
            save(&tx, &record, false)?;
            record
        }
        RoutineMutation::RunNow { id } => {
            let mut record = get(&tx, id)?;
            authorize(&tx, actor, record.spec.agent_id)?;
            if now_ms < 0 {
                return Err(RoutineError::Invalid("invalid occurrence time".to_owned()));
            }
            if pending_for(&tx, id)? {
                return Err(RoutineError::Busy);
            }
            insert_run(&tx, &record, operation, now_ms, now_ms)?;
            disarm_once(&mut record)?;
            save(&tx, &record, false)?;
            record
        }
    };
    active(cancellation)?;
    tx.execute(
        "INSERT INTO host_routine_mutations VALUES(?1,?2,?3,?4)",
        params![
            operation.to_string(),
            actor.to_string(),
            request,
            serde_json::to_string(&record)?
        ],
    )?;
    tx.commit()?;
    Ok(record)
}

// Disarming and admitting share a transaction. Bump the revision so an edit
// based on the armed state cannot accidentally re-arm a consumed occurrence.
fn disarm_once(record: &mut RoutineRecord) -> Result<bool, RoutineError> {
    if !matches!(record.spec.schedule, RoutineSchedule::Once { .. }) {
        return Ok(false);
    }
    if record.spec.enabled {
        record.spec.enabled = false;
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(RoutineError::Conflict)?;
    }
    Ok(true)
}

pub(super) fn authorize(
    db: &Connection,
    actor: AgentId,
    target: AgentId,
) -> Result<(), RoutineError> {
    let allowed: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM host_agents a JOIN host_bots b ON b.agent_id=?2 WHERE a.agent_id=?1 AND (a.agent_id=b.agent_id OR a.profile_id=?3))",params![actor.to_string(),target.to_string(),crate::ARCEE_PROFILE_ID],|row| row.get(0))?;
    if allowed {
        Ok(())
    } else {
        Err(RoutineError::Invalid(
            "only Arcee or the specialist itself can manage its routines".to_owned(),
        ))
    }
}

fn save(tx: &Transaction<'_>, r: &RoutineRecord, create: bool) -> Result<(), RoutineError> {
    let sql = if create {
        "INSERT INTO host_routines(id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
    } else {
        "UPDATE host_routines SET agent_id=?2,name=?3,prompt=?4,schedule_json=?5,enabled=?6,revision=?7,next_due_ms=?8 WHERE id=?1"
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

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRecord> {
    Ok(RoutineRecord {
        id: parse(row, 0)?,
        spec: RoutineSpec {
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
pub(super) fn get(db: &Connection, id: Uuid) -> Result<RoutineRecord, RoutineError> {
    db.query_row("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_routines WHERE id=?1",[id.to_string()],record).optional()?.ok_or(RoutineError::NotFound)
}
pub(super) fn list(
    path: &Path,
    agent: AgentId,
    after: Option<Uuid>,
) -> Result<Vec<RoutineRecord>, RoutineError> {
    let db = catalog::open_verified(path)?;
    let mut query=db.prepare("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_routines WHERE agent_id=?1 AND id>?2 ORDER BY id LIMIT 20")?;
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
fn pending_for(db: &Connection, id: Uuid) -> Result<bool, RoutineError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_routine_runs WHERE routine_id=?1 AND output IS NULL)",
        [id.to_string()],
        |row| row.get(0),
    )?)
}
pub(super) fn stable_id(value: &str) -> Uuid {
    use sha2::{Digest as _, Sha256};
    let hash = Sha256::digest(value.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    Uuid::from_bytes(bytes)
}
fn insert_run(
    tx: &Transaction<'_>,
    r: &RoutineRecord,
    id: Uuid,
    due: i64,
    admitted_at: i64,
) -> Result<(), RoutineError> {
    let session = stable_id(&format!("renoa.routine.session.v1:{}", r.id));
    tx.execute("INSERT INTO host_routine_runs(id,routine_id,agent_id,session_id,due_ms,admitted_at_ms,prompt) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id.to_string(),r.id.to_string(),r.spec.agent_id.to_string(),session.to_string(),due,admitted_at,r.spec.prompt])?;
    Ok(())
}
pub(super) fn run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRun> {
    Ok(RoutineRun {
        sequence: row.get(0)?,
        id: parse(row, 1)?,
        routine_id: parse(row, 2)?,
        agent_id: AgentId::from_uuid(parse(row, 3)?),
        session_id: parse(row, 4)?,
        due_ms: row.get(5)?,
        admitted_at_ms: row.get(6)?,
        prompt: row.get(7)?,
        output: row.get(8)?,
    })
}

/// Called only while holding the Host scheduler process lease. Admission and
/// advancing the clock commit together; unfinished runs retain their command ID.
pub(super) fn next(path: &Path, now_ms: i64) -> Result<Option<RoutineRun>, RoutineError> {
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let pending=tx.query_row("SELECT sequence,id,routine_id,agent_id,session_id,due_ms,admitted_at_ms,prompt,output FROM host_routine_runs WHERE output IS NULL ORDER BY sequence LIMIT 1",[],run).optional()?;
    if pending.is_some() {
        return Ok(pending);
    }
    let due=tx.query_row("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_routines WHERE enabled=1 AND next_due_ms<=?1 ORDER BY next_due_ms,id LIMIT 1",[now_ms],record).optional()?;
    let Some(mut r) = due else { return Ok(None) };
    let id = stable_id(&format!(
        "renoa.routine.occurrence.v1:{}:{}:{}",
        r.id, r.revision, r.next_due_ms
    ));
    insert_run(&tx, &r, id, r.next_due_ms, now_ms)?;
    // Coalesce missed times into one occurrence, then resume from the current clock.
    if !disarm_once(&mut r)? {
        r.next_due_ms = r.spec.schedule.advance_past(r.next_due_ms, now_ms)?;
    }
    save(&tx, &r, false)?;
    let admitted=tx.query_row("SELECT sequence,id,routine_id,agent_id,session_id,due_ms,admitted_at_ms,prompt,output FROM host_routine_runs WHERE id=?1",[id.to_string()],run)?;
    tx.commit()?;
    Ok(Some(admitted))
}

pub(super) fn finish(path: &Path, id: Uuid, output: &str) -> Result<(), RoutineError> {
    let db = catalog::open_verified(path)?;
    if db.execute(
        "UPDATE host_routine_runs SET output=?2 WHERE id=?1 AND output IS NULL",
        params![id.to_string(), output],
    )? != 1
    {
        return Err(RoutineError::Conflict);
    }
    Ok(())
}
pub(super) fn completed(path: &Path, after: i64) -> Result<Vec<RoutineRun>, RoutineError> {
    let db = catalog::open_verified(path)?;
    let mut q=db.prepare("SELECT sequence,id,routine_id,agent_id,session_id,due_ms,admitted_at_ms,prompt,output FROM host_routine_runs r WHERE sequence>?1 AND output IS NOT NULL AND NOT EXISTS(SELECT 1 FROM host_routine_runs earlier WHERE earlier.sequence<r.sequence AND earlier.output IS NULL) ORDER BY sequence LIMIT 20")?;
    Ok(q.query_map([after], run)?.collect::<Result<Vec<_>, _>>()?)
}
