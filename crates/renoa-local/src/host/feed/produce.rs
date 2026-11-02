//! The facts that write items, each inside the transaction that records it.
//!
//! Host-written text comes from fixed templates filled only with an
//! automation's name or a plugin's display name, so no error text, tool name,
//! URL, digest or model name reaches an item. The agent's name is not stored:
//! it is read when listing, so a rename shows.

use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

use super::item::{BODY_CHARS, FeedAction, FeedKind, TITLE_CHARS, fit};
use crate::{
    host::automations::{RunLanding, RunStatus},
    plugins::host::{HostPluginId, triggers::PLUGIN_OWNED},
    stable_id::stable_id,
};

/// How an agent's automation run ended, as the feed hears it.
#[derive(Clone, Copy, Debug)]
pub(in crate::host) enum RunEnded {
    Succeeded(RunLanding),
    Failed,
    Skipped,
}

/// Reports the end of `run` of `automation`, already recorded in `tx`. A
/// plugin's automation writes nothing: its runs are the plugin's.
///
/// A failure or skip opens a problem when it starts a failure streak. A
/// success marks the automation's open problem done and, when it ran in the
/// automation's own task, replaces the automation's result: a result in a
/// conversation is already there.
pub(in crate::host) fn run_ended(
    tx: &Connection,
    run: Uuid,
    automation: Uuid,
    ended: RunEnded,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let found = tx
        .query_row(
            &format!("SELECT agent_id, name FROM host_automations WHERE id = ?1 AND id NOT IN ({PLUGIN_OWNED})"),
            [automation.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((agent, name)) = found else {
        return Ok(());
    };
    let automation = automation.to_string();
    let run_id = run.to_string();
    let name = fit(&name, TITLE_CHARS - 20);
    let (kind, action, subject, title, body) = match ended {
        RunEnded::Succeeded(landing) => {
            tx.execute(
                "UPDATE host_feed_items SET done_at_ms = ?3
                 WHERE subject_id = ?1 AND kind = ?2 AND done_at_ms IS NULL",
                params![automation, FeedKind::Problem.as_str(), now_ms],
            )?;
            if landing == RunLanding::Conversation {
                return Ok(());
            }
            tx.execute(
                "DELETE FROM host_feed_items WHERE kind = ?1 AND subject_id != ?3
                 AND subject_id IN (SELECT id FROM host_automation_runs WHERE automation_id = ?2)",
                params![FeedKind::Result.as_str(), automation, run_id],
            )?;
            (
                FeedKind::Result,
                FeedAction::Open,
                &run_id,
                format!("\u{201c}{name}\u{201d} finished"),
                "Open it to read what it found.",
            )
        }
        RunEnded::Failed | RunEnded::Skipped if !starts_failure_streak(tx, run)? => {
            return Ok(());
        }
        RunEnded::Failed => (
            FeedKind::Problem,
            FeedAction::Retry,
            &automation,
            format!("\u{201c}{name}\u{201d} didn\u{2019}t finish"),
            "Something went wrong on this run. You can run it again now.",
        ),
        RunEnded::Skipped => (
            FeedKind::Problem,
            FeedAction::Retry,
            &automation,
            format!("\u{201c}{name}\u{201d} was skipped"),
            "It started too late to be worth running. You can run it now.",
        ),
    };
    insert(
        tx,
        &NewItem {
            source: &run_id,
            kind,
            agent: &agent,
            title: &title,
            body,
            action,
            subject,
        },
        Some(now_ms),
    )
}

/// Whether finished `run` starts its automation's failure streak: it failed
/// or was skipped, and the automation's previous finished run did neither.
/// One streak is reported once.
pub(crate) fn starts_failure_streak(db: &Connection, run: Uuid) -> rusqlite::Result<bool> {
    let (status, previous) = db.query_row(
        "SELECT r.status, (SELECT p.status FROM host_automation_runs p
             WHERE p.automation_id = r.automation_id AND p.sequence < r.sequence
               AND p.output IS NOT NULL
             ORDER BY p.sequence DESC LIMIT 1)
         FROM host_automation_runs r WHERE r.id = ?1",
        [run.to_string()],
        |row| {
            Ok((
                row.get::<_, Option<RunStatus>>(0)?,
                row.get::<_, Option<RunStatus>>(1)?,
            ))
        },
    )?;
    let failing = |status: Option<RunStatus>| match status {
        Some(RunStatus::Failed | RunStatus::Skipped) => true,
        Some(RunStatus::Succeeded) | None => false,
    };
    Ok(failing(status) && !failing(previous))
}

/// An ability an agent can turn on.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ability<'a> {
    /// An installed plugin's activation, by its identity and selected revision.
    Package { plugin_id: &'a str, digest: &'a str },
    /// A plugin compiled into the Host.
    Compiled(HostPluginId),
}

/// Reports that `agent` turned `ability` on for itself, by the act `source`
/// names once. The owner turning it on, or another agent doing it for this
/// one, is not reported: callers write this only for an agent's own act.
/// The item's time is the catalog's clock, read in the same transaction.
pub(crate) fn ability_turned_on(
    tx: &Connection,
    agent: &str,
    ability: Ability<'_>,
    source: &str,
) -> rusqlite::Result<()> {
    let (subject, name) = match ability {
        Ability::Package { plugin_id, digest } => (
            plugin_id,
            tx.query_row(
                "SELECT name FROM installed_plugins WHERE plugin_digest = ?1",
                [digest],
                |row| row.get::<_, String>(0),
            )?,
        ),
        Ability::Compiled(plugin) => (plugin.id(), plugin.display_name().to_owned()),
    };
    insert(
        tx,
        &NewItem {
            source: &format!("{agent}:{source}"),
            kind: FeedKind::Ability,
            agent,
            title: &format!("Turned on {}", fit(&name, TITLE_CHARS - 10)),
            body: "It added this to its own abilities. You can turn it off.",
            action: FeedAction::TurnOff,
            subject,
        },
        None,
    )
}

/// Marks item `id` done at `now_ms` unless it is already. Returns whether the
/// item exists. A producer marks its own item done this way, as the owner's
/// dismissal does.
pub(super) fn done(tx: &Connection, id: Uuid, now_ms: i64) -> rusqlite::Result<bool> {
    let changed = tx.execute(
        "UPDATE host_feed_items SET done_at_ms = ?2 WHERE id = ?1 AND done_at_ms IS NULL",
        params![id.to_string(), now_ms],
    )?;
    if changed > 0 {
        return Ok(true);
    }
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_feed_items WHERE id = ?1)",
        [id.to_string()],
        |row| row.get(0),
    )
}

struct NewItem<'a> {
    /// What the item reports, unique within its kind.
    source: &'a str,
    kind: FeedKind,
    agent: &'a str,
    title: &'a str,
    body: &'a str,
    action: FeedAction,
    subject: &'a str,
}

/// Inserts `item` once under the identity its kind and source derive. A
/// `None` time is the catalog's clock.
fn insert(tx: &Connection, item: &NewItem<'_>, at_ms: Option<i64>) -> rusqlite::Result<()> {
    let id = stable_id(&format!(
        "renoa.feed.v1:{}:{}",
        item.kind.as_str(),
        item.source
    ));
    tx.execute(
        "INSERT OR IGNORE INTO host_feed_items(id, kind, agent_id, title, body, action, subject_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                 coalesce(?8, CAST((julianday('now') - 2440587.5) * 86400000.0 AS INTEGER)))",
        params![
            id.to_string(),
            item.kind.as_str(),
            item.agent,
            fit(item.title, TITLE_CHARS),
            fit(item.body, BODY_CHARS),
            item.action.as_str(),
            item.subject,
            at_ms,
        ],
    )?;
    Ok(())
}
