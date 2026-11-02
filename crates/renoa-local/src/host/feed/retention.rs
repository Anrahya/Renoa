//! How long items live. An item is kept [`KEEP_ITEMS_MS`] from its creation,
//! the window the runs it describes already have, and only the newest
//! [`KEEP_ITEMS`] are kept. An open item that needs the owner, an ask or a
//! problem, is exempt from both: nothing else tells the owner an agent is
//! waiting or an automation still fails. Nothing is archived; each removal is
//! reported as one telemetry event with its counts and no text.

use rusqlite::{Connection, params};
use serde_json::json;
use uuid::Uuid;

use super::{FeedKind, item::needing_owner};
use crate::host::automations::Removed;

/// Items kept at most.
pub(in crate::host) const KEEP_ITEMS: i64 = 200;
/// The longest an item is kept.
pub(in crate::host) const KEEP_ITEMS_MS: i64 = 30 * 24 * 3_600_000;

/// The items retention never removes: the open ones that need the owner.
fn exempt() -> String {
    format!("(kind IN ({}) AND done_at_ms IS NULL)", needing_owner())
}

/// Removes items past their age, then all but the newest [`KEEP_ITEMS`].
pub(in crate::host) fn expire(db: &Connection, now_ms: i64) -> rusqlite::Result<Vec<Removed>> {
    let exempt = exempt();
    let aged = db.execute(
        &format!("DELETE FROM host_feed_items WHERE NOT {exempt} AND created_at_ms < ?1"),
        [now_ms.saturating_sub(KEEP_ITEMS_MS)],
    )?;
    let over = db.execute(
        &format!(
            "DELETE FROM host_feed_items WHERE NOT {exempt} AND sequence NOT IN
                (SELECT sequence FROM host_feed_items ORDER BY sequence DESC LIMIT ?1)"
        ),
        [KEEP_ITEMS],
    )?;
    Ok([(aged, "age"), (over, "count")]
        .into_iter()
        .filter(|(items, _)| *items > 0)
        .map(|(items, reason)| {
            Removed::new(
                "feed_items_pruned",
                json!({ "items": items, "reason": reason }),
            )
        })
        .collect())
}

/// Removes every item about `automation` or one of its runs, since their
/// text carries its name. Runs before its runs are removed.
pub(in crate::host) fn forget_automation(
    db: &Connection,
    automation: Uuid,
) -> rusqlite::Result<usize> {
    db.execute(
        "DELETE FROM host_feed_items WHERE subject_id = ?1
            OR subject_id IN (SELECT id FROM host_automation_runs WHERE automation_id = ?1)",
        [automation.to_string()],
    )
}

/// Removes the results of `runs`, which are being removed: a result that
/// cannot be opened is not kept.
pub(in crate::host) fn forget_results(db: &Connection, runs: &[String]) -> rusqlite::Result<usize> {
    let mut removed = 0;
    let mut statement =
        db.prepare_cached("DELETE FROM host_feed_items WHERE kind = ?1 AND subject_id = ?2")?;
    for run in runs {
        removed += statement.execute(params![FeedKind::Result.as_str(), run])?;
    }
    Ok(removed)
}
