//! A run's life on the Host: admission of a due occurrence, the lateness rule,
//! and the outcome its executor reports.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use uuid::Uuid;

use super::{
    AutomationError, AutomationRecord, AutomationRun, RunOutcome, RunResult, RunStatus, retention,
    store,
};
use crate::{host::catalog, plugins::host::triggers::PLUGIN_OWNED};

mod unschedulable;

/// Every read of a run selects these columns, in the order [`run`] maps them.
pub(super) const RUN_COLUMNS: &str = "sequence,id,automation_id,agent_id,due_ms,admitted_at_ms,submission,output,status,failed_tool_calls,finished_at_ms";

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

/// Admits a run of `r`. A scheduled run has the time it was `due`; a run
/// requested with `run_now` has none and is due when admitted.
pub(super) fn insert_run(
    tx: &Connection,
    r: &AutomationRecord,
    id: Uuid,
    due: Option<i64>,
    admitted_at: i64,
) -> Result<(), AutomationError> {
    let submission = submission(r, due, admitted_at)?;
    tx.execute("INSERT INTO host_automation_runs(id,automation_id,agent_id,due_ms,admitted_at_ms,submission) VALUES(?1,?2,?3,?4,?5,?6)",params![id.to_string(),r.id.to_string(),r.spec.agent_id.to_string(),due.unwrap_or(admitted_at),admitted_at,submission])?;
    Ok(())
}

/// The standing task after one line of context: which automation this is,
/// when it was due, and how late it started when that exceeds
/// [`LATE_NOTE_AFTER_MS`].
fn submission(
    r: &AutomationRecord,
    due: Option<i64>,
    admitted_at: i64,
) -> Result<String, AutomationError> {
    let name = &r.spec.name;
    let mut context = match due {
        None => format!("Requested run of \"{name}\"."),
        Some(due) => match r.spec.schedule.due_label(due)? {
            Some(label) => format!("Scheduled run \"{name}\", due {label}."),
            None => format!("Scheduled run \"{name}\"."),
        },
    };
    let late = due.map_or(0, |due| admitted_at.saturating_sub(due));
    if late > LATE_NOTE_AFTER_MS {
        context = format!("{context} Started {} late.", duration(late));
    }
    Ok(format!("({context})\n\n{}", r.spec.prompt))
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
        submission: row.get(6)?,
        result,
    })
}

/// Called only while holding the Host scheduler process lease. Returns the
/// oldest unfinished run, or converges every plugin's trigger and admits the
/// next due occurrence. Admission and
/// advancing the clock commit together; unfinished runs retain their command
/// ID. An occurrence admitted later than its schedule allows is recorded as
/// skipped in the same transaction and returned finished, so the scheduler
/// reports it and asks again. While `plugin_running`, a plugin's run is in
/// flight beside the scheduler, so no plugin's run is handed on or admitted:
/// one plugin run at a time, and an agent's due run is not held up by it. A
/// pending run whose row does not decode, or a due automation whose row does
/// not decode or whose schedule cannot be computed, is left unchanged and
/// logged as `automation_unschedulable`, at most every ten minutes, and the
/// next one is taken, so one bad row does not stop the others.
pub(super) fn next(
    path: &Path,
    now_ms: i64,
    plugin_running: bool,
) -> Result<Option<AutomationRun>, AutomationError> {
    let mut db = catalog::open_verified(path)?;
    let mut tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(pending) = pending(&tx, plugin_running)? {
        return Ok(Some(pending));
    }
    super::owned::converge(&tx, now_ms)?;
    let due = tx
        .prepare(&format!("SELECT id FROM host_automations WHERE enabled=1 AND next_due_ms<=?1 AND NOT EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id=host_automations.id) AND NOT (?2 AND id IN ({PLUGIN_OWNED})) ORDER BY next_due_ms,id"))?
        .query_map(params![now_ms, plugin_running], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    // One automation whose row cannot be read or scheduled is left as it is
    // and logged, so it does not hold up the others.
    for automation in due {
        let mut attempt = tx.savepoint()?;
        match admit(&attempt, &automation, now_ms) {
            Ok((admitted, pruned)) => {
                attempt.commit()?;
                tx.commit()?;
                retention::report(pruned);
                return Ok(Some(admitted));
            }
            Err(error) if of_its_row(&error) => {
                attempt.rollback()?;
                unschedulable::report("automation_id", &automation, &error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    // The converged triggers stand whether or not a run is due.
    tx.commit()?;
    Ok(None)
}

/// The oldest run without a result whose row decodes, as [`next`] describes.
fn pending(
    tx: &Connection,
    plugin_running: bool,
) -> Result<Option<AutomationRun>, AutomationError> {
    let mut pending = tx.prepare(&format!("SELECT {RUN_COLUMNS} FROM host_automation_runs WHERE output IS NULL AND NOT (?1 AND automation_id IN ({PLUGIN_OWNED})) ORDER BY sequence"))?;
    let mut rows = pending.query([plugin_running])?;
    while let Some(row) = rows.next()? {
        match run(row) {
            Ok(found) => return Ok(Some(found)),
            Err(error) if decodes_its_row(&error) => {
                let id = row.get::<_, String>(1).unwrap_or_default();
                unschedulable::report("run_id", &id, &error.to_string());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

/// Admits the due occurrence of `automation`, as [`next`] describes, and
/// returns the run with the runs retention removed.
fn admit(
    tx: &Connection,
    automation: &str,
    now_ms: i64,
) -> Result<(AutomationRun, Option<retention::Removed>), AutomationError> {
    let mut r = tx.query_row(
        "SELECT id,agent_id,name,prompt,schedule_json,enabled,revision,next_due_ms FROM host_automations WHERE id=?1",
        [automation],
        store::record,
    )?;
    let id = crate::stable_id::stable_id(&format!(
        "renoa.automation.occurrence.v1:{}:{}:{}",
        r.id, r.revision, r.next_due_ms
    ));
    insert_run(tx, &r, id, Some(r.next_due_ms), now_ms)?;
    let late = now_ms.saturating_sub(r.next_due_ms);
    if let Some(limit) = r.spec.schedule.skip_after_ms(r.next_due_ms)?
        && late > limit
    {
        let reason = format!(
            "Scheduled run skipped: it reached the scheduler {} after its due time, and a run of this schedule is skipped once it is more than {} late.",
            duration(late),
            duration(limit)
        );
        record(tx, id, RunStatus::Skipped, &reason, None, now_ms)?;
    }
    let pruned = retention::keep_newest(tx, r.id)?;
    // Coalesce missed times into one occurrence, then resume from the current clock.
    if !store::disarm_once(&mut r)? {
        r.next_due_ms = r.spec.schedule.next_after(now_ms)?;
    }
    store::save(tx, &r, false)?;
    let admitted = tx.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM host_automation_runs WHERE id=?1"),
        [id.to_string()],
        run,
    )?;
    Ok((admitted, pruned))
}

/// Whether `error` is about one automation's stored row rather than the
/// catalog: its row does not decode, or its schedule cannot be computed.
fn of_its_row(error: &AutomationError) -> bool {
    match error {
        AutomationError::Database(error) => decodes_its_row(error),
        AutomationError::Invalid(_) | AutomationError::Json(_) | AutomationError::Time(_) => true,
        AutomationError::Conflict
        | AutomationError::NotFound
        | AutomationError::Busy
        | AutomationError::Cancelled
        | AutomationError::Forbidden
        | AutomationError::Catalog(_) => false,
    }
}

/// Whether `error` is a stored value its column cannot be read as.
fn decodes_its_row(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::Utf8Error(..)
    )
}

/// Records an executed run's outcome, which ends it; see [`end`].
pub(super) fn finish(
    path: &Path,
    id: Uuid,
    outcome: &RunOutcome,
    now_ms: i64,
) -> Result<(), AutomationError> {
    let (RunOutcome::Succeeded { answer: output, .. } | RunOutcome::Failed { reason: output, .. }) =
        outcome;
    end(
        path,
        id,
        outcome.status(),
        output,
        Some(outcome.failed_tool_calls()),
        now_ms,
    )
}

/// Ends an unfinished run with `status`, then applies its automation's
/// retention: the run limit, and the purge a deletion waited for. Only the
/// Host skips: an executor reports through [`finish`], and a plugin that owns
/// the run decides its status itself.
pub(super) fn end(
    path: &Path,
    id: Uuid,
    status: RunStatus,
    output: &str,
    failed_tool_calls: Option<u32>,
    now_ms: i64,
) -> Result<(), AutomationError> {
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction()?;
    let automation = record(&tx, id, status, output, failed_tool_calls, now_ms)?;
    let removed = [
        retention::keep_newest(&tx, automation)?,
        retention::purge_deleted(&tx, automation)?,
    ];
    tx.commit()?;
    retention::report(removed.into_iter().flatten());
    Ok(())
}

/// Ends an unfinished run and returns its automation.
fn record(
    db: &Connection,
    id: Uuid,
    status: RunStatus,
    output: &str,
    failed_tool_calls: Option<u32>,
    now_ms: i64,
) -> Result<Uuid, AutomationError> {
    let automation = db
        .query_row(
            "UPDATE host_automation_runs SET output=?2,status=?3,failed_tool_calls=?4,finished_at_ms=?5 WHERE id=?1 AND output IS NULL RETURNING automation_id",
            params![id.to_string(), output, status.as_str(), failed_tool_calls, now_ms],
            |row| store::parse(row, 0),
        )
        .optional()?;
    automation.ok_or(AutomationError::Conflict)
}

/// Whether `id` is an admitted run of an agent's automation with no result
/// yet: one the scheduler is delivering, under `id`, and waits for.
pub(crate) fn delivering(path: &Path, id: Uuid) -> Result<bool, AutomationError> {
    let db = catalog::open_verified(path)?;
    Ok(db.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM host_automation_runs
             WHERE id = ?1 AND output IS NULL AND automation_id NOT IN ({PLUGIN_OWNED}))"
        ),
        [id.to_string()],
        |row| row.get(0),
    )?)
}

pub(super) fn completed(path: &Path, after: i64) -> Result<Vec<AutomationRun>, AutomationError> {
    let db = catalog::open_verified(path)?;
    let mut q=db.prepare(&format!("SELECT {RUN_COLUMNS} FROM host_automation_runs r WHERE sequence>?1 AND output IS NOT NULL AND automation_id NOT IN ({PLUGIN_OWNED}) AND NOT EXISTS(SELECT 1 FROM host_automation_runs earlier WHERE earlier.sequence<r.sequence AND earlier.output IS NULL AND earlier.automation_id NOT IN ({PLUGIN_OWNED})) ORDER BY sequence LIMIT 20"))?;
    Ok(q.query_map([after], run)?.collect::<Result<Vec<_>, _>>()?)
}

/// Records that the process owning the schedule is alive, and expires runs
/// past their age on the same beat.
pub(super) fn heartbeat(path: &Path, now_ms: i64) -> Result<(), AutomationError> {
    let db = catalog::open_verified(path)?;
    db.execute(
        "INSERT INTO host_automation_scheduler(singleton,heartbeat_ms) VALUES(1,?1)
         ON CONFLICT(singleton) DO UPDATE SET heartbeat_ms=excluded.heartbeat_ms",
        [now_ms],
    )?;
    retention::report(retention::expire(&db, now_ms)?);
    Ok(())
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

#[cfg(test)]
mod tests {
    use rusqlite::{ffi, types::Type};

    use super::{AutomationError, of_its_row};

    #[test]
    fn every_value_its_column_cannot_be_read_as_is_its_rows_and_a_catalog_failure_is_not() {
        let invalid = vec![0xff_u8];
        let utf8 = std::str::from_utf8(&invalid).expect_err("not UTF-8");
        let of_a_row = [
            rusqlite::Error::FromSqlConversionFailure(0, Type::Text, "bad".into()),
            rusqlite::Error::InvalidColumnType(0, "name".to_owned(), Type::Blob),
            rusqlite::Error::IntegralValueOutOfRange(0, i64::MAX),
            rusqlite::Error::from(utf8),
        ];
        for error in of_a_row {
            assert!(of_its_row(&AutomationError::Database(error)));
        }
        let busy = rusqlite::Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_BUSY), None);
        assert!(!of_its_row(&AutomationError::Database(busy)));
        assert!(!of_its_row(&AutomationError::Database(
            rusqlite::Error::QueryReturnedNoRows
        )));
    }
}
