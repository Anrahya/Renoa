//! How long automation data lives. Each automation keeps its newest
//! [`KEEP_RUNS`] finished runs, none older than [`KEEP_RUNS_MS`], and a deleted
//! automation keeps nothing but the fact that it existed once its last run has
//! finished. Nothing is archived; every removal is reported as one telemetry
//! event with its counts.

use rusqlite::{Connection, Transaction};
use serde_json::json;
use uuid::Uuid;

use super::AutomationError;

/// Finished runs kept per automation.
pub(super) const KEEP_RUNS: i64 = 50;
/// The longest a finished run is kept.
pub(super) const KEEP_RUNS_MS: i64 = 30 * 24 * 3_600_000;

const LOG_COMPONENT: &str = "renoa.host";

/// A removal to report once its transaction has committed.
#[must_use]
pub(in crate::host) struct Removed {
    name: &'static str,
    fields: serde_json::Value,
}

/// Reports committed removals.
pub(in crate::host) fn report(removed: impl IntoIterator<Item = Removed>) {
    for removed in removed {
        renoa_telemetry::event(LOG_COMPONENT, "info", removed.name, &removed.fields);
    }
}

/// Removes an automation's finished runs beyond its newest [`KEEP_RUNS`].
pub(super) fn keep_newest(
    tx: &Transaction<'_>,
    automation: Uuid,
) -> Result<Option<Removed>, AutomationError> {
    let runs = tx.execute(
        "DELETE FROM host_automation_runs WHERE automation_id=?1 AND output IS NOT NULL
         AND sequence < (SELECT min(sequence) FROM (SELECT sequence FROM host_automation_runs
             WHERE automation_id=?1 AND output IS NOT NULL ORDER BY sequence DESC LIMIT ?2))",
        rusqlite::params![automation.to_string(), KEEP_RUNS],
    )?;
    Ok((runs > 0).then(|| Removed {
        name: "automation_runs_pruned",
        fields: json!({ "automation_id": automation, "runs": runs, "reason": "count" }),
    }))
}

/// Removes every finished run older than [`KEEP_RUNS_MS`]. A run recorded
/// before schema 37 has no finishing time and ages from its admission.
pub(super) fn expire(db: &Connection, now_ms: i64) -> Result<Option<Removed>, AutomationError> {
    let runs = db.execute(
        "DELETE FROM host_automation_runs WHERE output IS NOT NULL
         AND coalesce(finished_at_ms, admitted_at_ms) < ?1",
        [now_ms.saturating_sub(KEEP_RUNS_MS)],
    )?;
    Ok((runs > 0).then(|| Removed {
        name: "automation_runs_pruned",
        fields: json!({ "runs": runs, "reason": "age" }),
    }))
}

/// Removes what a deleted automation still holds once no run of it is in
/// flight: its runs, and its name and standing task wherever they are kept.
/// Its row, deletion mark and receipts stay without them, so a retried
/// operation still gets its original answer and cannot bring it back: a
/// receipt keeps its request only as a digest (see [`request_matches`]) and
/// its result with the name and task blank. Its own conversation is marked
/// for the schedule's owner to delete (see [`conversations_to_delete`]).
/// Returns `None` while it is not deleted or a run is unfinished, since the
/// run's finish removes it then, and when nothing was left to remove.
pub(super) fn purge_deleted(
    tx: &Transaction<'_>,
    automation: Uuid,
) -> Result<Option<Removed>, AutomationError> {
    let id = automation.to_string();
    let ready: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id=?1)
            AND NOT EXISTS(SELECT 1 FROM host_automation_runs WHERE automation_id=?1 AND output IS NULL)",
        [&id],
        |row| row.get(0),
    )?;
    if !ready {
        return Ok(None);
    }
    let runs = tx.execute(
        "DELETE FROM host_automation_runs WHERE automation_id=?1",
        [&id],
    )?;
    let marked = tx.execute(
        "INSERT OR IGNORE INTO host_automation_conversation_deletions(automation_id) VALUES(?1)",
        [&id],
    )?;
    tx.execute(
        "UPDATE host_automations SET name='', prompt='' WHERE id=?1",
        [&id],
    )?;
    let mut receipts = 0;
    for table in [
        "host_automation_mutations",
        "host_automation_owner_mutations",
    ] {
        let requests = tx
            .prepare(&format!(
                "SELECT operation_id, request_json FROM {table}
                 WHERE json_extract(result_json, '$.id')=?1
                   AND json_extract(request_json, '$.sha256') IS NULL"
            ))?
            .query_map([&id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (operation, request) in requests {
            receipts += tx.execute(
                &format!(
                    "UPDATE {table} SET request_json=json_object('sha256', ?2),
                        result_json=json_set(result_json, '$.spec.name', '', '$.spec.prompt', '')
                     WHERE operation_id=?1"
                ),
                [operation, digest(&request)],
            )?;
        }
    }
    if runs == 0 && receipts == 0 && marked == 0 {
        return Ok(None);
    }
    Ok(Some(Removed {
        name: "automation_purged",
        fields: json!({ "automation_id": automation, "runs": runs, "receipts": receipts }),
    }))
}

/// Purges every deleted automation with no run in flight. Schema 39 uses it
/// for automations deleted before deletion removed their data.
pub(in crate::host) fn purge_all_deleted(
    tx: &Transaction<'_>,
) -> Result<Vec<Removed>, AutomationError> {
    let deleted = tx
        .prepare("SELECT automation_id FROM host_automation_deletions")?
        .query_map([], |row| super::store::parse(row, 0))?
        .collect::<Result<Vec<Uuid>, _>>()?;
    let mut removed = Vec::new();
    for automation in deleted {
        removed.extend(purge_deleted(tx, automation)?);
    }
    Ok(removed)
}

/// The indexes run history is read and pruned through: per automation for
/// its counts, latest run and limit, per agent for its results.
pub(in crate::host) const RUN_INDEXES: &str =
    "CREATE INDEX IF NOT EXISTS host_automation_runs_by_automation
         ON host_automation_runs(automation_id, sequence);
     CREATE INDEX IF NOT EXISTS host_automation_runs_by_agent
         ON host_automation_runs(agent_id, sequence);";

fn digest(request: &str) -> String {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(request.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
            hex
        })
}

/// Whether a retried request is the one a receipt recorded, in full or, once
/// its automation was purged, as a digest.
pub(super) fn request_matches(stored: &str, request: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(stored)
        .ok()
        .as_ref()
        .and_then(|stored| stored.get("sha256"))
        .and_then(serde_json::Value::as_str)
    {
        Some(sha256) => sha256 == digest(request),
        None => stored == request,
    }
}

/// Deleted automations whose own conversation, the RCP task named by the
/// automation's id and its session, may still exist.
pub(in crate::host) const CONVERSATION_DELETIONS: &str =
    "CREATE TABLE IF NOT EXISTS host_automation_conversation_deletions (
        automation_id TEXT PRIMARY KEY REFERENCES host_automations(id)
    ) STRICT;";

/// The purged automations whose conversation is still to be deleted.
pub(super) fn conversations_to_delete(db: &Connection) -> Result<Vec<Uuid>, AutomationError> {
    Ok(db
        .prepare("SELECT automation_id FROM host_automation_conversation_deletions")?
        .query_map([], |row| super::store::parse(row, 0))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Records that an automation's conversation no longer exists anywhere.
pub(super) fn conversation_deleted(
    db: &Connection,
    automation: Uuid,
) -> Result<(), AutomationError> {
    db.execute(
        "DELETE FROM host_automation_conversation_deletions WHERE automation_id=?1",
        [automation.to_string()],
    )?;
    Ok(())
}
