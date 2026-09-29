//! A run's life on the Host: admission of a due occurrence, the lateness rule,
//! and the outcome its executor reports.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use super::{
    AutomationError, AutomationRecord, AutomationRun, RunOutcome, RunResult, RunStatus, store,
};
use crate::host::catalog;

/// Every read of a run selects these columns, in the order [`run`] maps them.
pub(super) const RUN_COLUMNS: &str = "sequence,id,automation_id,agent_id,due_ms,admitted_at_ms,prompt,output,status,failed_tool_calls,finished_at_ms";

/// The liveness record of the process owning the schedule, which writes its
/// [`heartbeat`] while it runs. Fresh catalogs and the schema 37 upgrade share
/// it; the schema 33 rename step also runs the fresh definitions, so it must be
/// idempotent.
pub(in crate::host) const SCHEDULER_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS host_automation_scheduler (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1), heartbeat_ms INTEGER NOT NULL
) STRICT;";

/// A run that starts later than this after its due time tells the agent so.
const LATE_NOTE_AFTER_MS: i64 = 5 * 60_000;

pub(super) fn insert_run(
    tx: &Transaction<'_>,
    r: &AutomationRecord,
    id: Uuid,
    due: i64,
    admitted_at: i64,
) -> Result<(), AutomationError> {
    tx.execute("INSERT INTO host_automation_runs(id,automation_id,agent_id,due_ms,admitted_at_ms,prompt) VALUES(?1,?2,?3,?4,?5,?6)",params![id.to_string(),r.id.to_string(),r.spec.agent_id.to_string(),due,admitted_at,r.spec.prompt])?;
    Ok(())
}

/// Maps a row selected with [`RUN_COLUMNS`]. A finished run has both a status
/// and an output and a pending run has neither; a row with only one is corrupt.
pub(super) fn run(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationRun> {
    let result = match (row.get::<_, Option<RunStatus>>(8)?, row.get(7)?) {
        (None, None) => None,
        (Some(status), Some(output)) => Some(RunResult {
            status,
            output,
            failed_tool_calls: row.get(9)?,
            finished_at_ms: row.get(10)?,
        }),
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                "an automation run's status and output must be recorded together".into(),
            ));
        }
    };
    Ok(AutomationRun {
        sequence: row.get(0)?,
        id: store::parse(row, 1)?,
        automation_id: store::parse(row, 2)?,
        agent_id: AgentId::from_uuid(store::parse(row, 3)?),
        due_ms: row.get(4)?,
        admitted_at_ms: row.get(5)?,
        prompt: row.get(6)?,
        result,
    })
}

/// Called only while holding the Host scheduler process lease. Returns the
/// oldest unfinished run, or admits the next due occurrence. Admission and
/// advancing the clock commit together; unfinished runs retain their command
/// ID. An occurrence admitted later than its schedule allows is recorded as
/// skipped in the same transaction and returned finished, so the scheduler
/// reports it and asks again.
pub(super) fn next(path: &Path, now_ms: i64) -> Result<Option<AutomationRun>, AutomationError> {
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let pending = tx
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM host_automation_runs WHERE output IS NULL ORDER BY sequence LIMIT 1"),
            [],
            run,
        )
        .optional()?;
    if pending.is_some() {
        return Ok(pending);
    }
    let due=tx.query_row("SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_automations WHERE enabled=1 AND next_due_ms<=?1 AND NOT EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id=host_automations.id) ORDER BY next_due_ms,id LIMIT 1",[now_ms],store::record).optional()?;
    let Some(mut r) = due else { return Ok(None) };
    let id = crate::stable_id::stable_id(&format!(
        "renoa.automation.occurrence.v1:{}:{}:{}",
        r.id, r.revision, r.next_due_ms
    ));
    insert_run(&tx, &r, id, r.next_due_ms, now_ms)?;
    let late = now_ms.saturating_sub(r.next_due_ms);
    if let Some(limit) = r.spec.schedule.skip_after_ms()
        && late > limit
    {
        let reason = format!(
            "Scheduled run skipped: it reached the scheduler {} after its due time, and a run of this schedule is skipped once it is more than {} late.",
            duration(late),
            duration(limit)
        );
        record(&tx, id, RunStatus::Skipped, &reason, None, now_ms)?;
    }
    // Coalesce missed times into one occurrence, then resume from the current clock.
    if !store::disarm_once(&mut r)? {
        r.next_due_ms = r.spec.schedule.advance_past(r.next_due_ms, now_ms)?;
    }
    store::save(&tx, &r, false)?;
    let admitted = tx.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM host_automation_runs WHERE id=?1"),
        [id.to_string()],
        run,
    )?;
    tx.commit()?;
    Ok(Some(admitted))
}

/// Records an executed run's outcome, which ends it.
pub(super) fn finish(
    path: &Path,
    id: Uuid,
    outcome: &RunOutcome,
    now_ms: i64,
) -> Result<(), AutomationError> {
    let (RunOutcome::Succeeded { answer: output, .. } | RunOutcome::Failed { reason: output, .. }) =
        outcome;
    record(
        &catalog::open_verified(path)?,
        id,
        outcome.status(),
        output,
        Some(outcome.failed_tool_calls()),
        now_ms,
    )
}

fn record(
    db: &Connection,
    id: Uuid,
    status: RunStatus,
    output: &str,
    failed_tool_calls: Option<u32>,
    now_ms: i64,
) -> Result<(), AutomationError> {
    if db.execute(
        "UPDATE host_automation_runs SET output=?2,status=?3,failed_tool_calls=?4,finished_at_ms=?5 WHERE id=?1 AND output IS NULL",
        params![id.to_string(), output, status.as_str(), failed_tool_calls, now_ms],
    )? != 1
    {
        return Err(AutomationError::Conflict);
    }
    Ok(())
}

pub(super) fn completed(path: &Path, after: i64) -> Result<Vec<AutomationRun>, AutomationError> {
    let db = catalog::open_verified(path)?;
    let mut q=db.prepare(&format!("SELECT {RUN_COLUMNS} FROM host_automation_runs r WHERE sequence>?1 AND output IS NOT NULL AND NOT EXISTS(SELECT 1 FROM host_automation_runs earlier WHERE earlier.sequence<r.sequence AND earlier.output IS NULL) ORDER BY sequence LIMIT 20"))?;
    Ok(q.query_map([after], run)?.collect::<Result<Vec<_>, _>>()?)
}

/// Records that the process owning the schedule is alive.
pub(super) fn heartbeat(path: &Path, now_ms: i64) -> Result<(), AutomationError> {
    catalog::open_verified(path)?.execute(
        "INSERT INTO host_automation_scheduler(singleton,heartbeat_ms) VALUES(1,?1)
         ON CONFLICT(singleton) DO UPDATE SET heartbeat_ms=excluded.heartbeat_ms",
        [now_ms],
    )?;
    Ok(())
}

impl AutomationRun {
    /// The text an executor submits for this run: its standing task, preceded
    /// by a note when it starts late. It depends only on the stored run, so a
    /// resubmission after a restart is the same command.
    #[must_use]
    pub fn submission(&self) -> String {
        let late = self.admitted_at_ms.saturating_sub(self.due_ms);
        if late <= LATE_NOTE_AFTER_MS {
            return self.prompt.clone();
        }
        format!(
            "(This scheduled run started {} after its due time.)\n\n{}",
            duration(late),
            self.prompt
        )
    }
}

/// Whole hours and minutes, the precision a late run is reported in.
fn duration(ms: i64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}
