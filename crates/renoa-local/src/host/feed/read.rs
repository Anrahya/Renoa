//! The owner's three feed operations: list a page, raise `seen_through`, and
//! dismiss an item.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::Serialize;
use uuid::Uuid;

use super::{FEED_PAGE, FeedAction, FeedError, FeedKind, item, newest, produce};
use crate::host::catalog;

/// One item as an app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedItem {
    pub id: Uuid,
    pub sequence: i64,
    pub kind: FeedKind,
    pub agent: FeedAgent,
    pub title: String,
    pub body: String,
    pub at_ms: i64,
    pub done: bool,
    pub action: FeedAction,
    /// What the action works on: the automation to retry, the run to open,
    /// or the plugin to turn off for `agent`.
    pub subject_id: String,
}

/// The agent an item is about, with its name as of this read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedAgent {
    pub id: AgentId,
    pub name: String,
}

/// One page of the feed and what Home shows above it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedPage {
    pub today: String,
    /// Open items above `seen_through`.
    pub unread: u32,
    pub seen_through: i64,
    pub items: Vec<FeedItem>,
    /// The `before` of the next older page; `None` on the oldest.
    pub next_before: Option<i64>,
}

/// The owner's read state after seeing the feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedSeen {
    pub seen_through: i64,
    pub unread: u32,
}

const ITEM_COLUMNS: &str = "i.id, i.sequence, i.kind, i.agent_id, a.name, i.title, i.body,
    i.created_at_ms, i.done_at_ms IS NOT NULL, i.action, i.subject_id
    FROM host_feed_items i JOIN host_agents a ON a.agent_id = i.agent_id";

fn item(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeedItem> {
    let uuid = |index: usize| {
        let value: String = row.get(index)?;
        Uuid::parse_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    Ok(FeedItem {
        id: uuid(0)?,
        sequence: row.get(1)?,
        kind: row.get(2)?,
        agent: FeedAgent {
            id: AgentId::from_uuid(uuid(3)?),
            name: row.get(4)?,
        },
        title: row.get(5)?,
        body: row.get(6)?,
        at_ms: row.get(7)?,
        done: row.get(8)?,
        action: row.get(9)?,
        subject_id: row.get(10)?,
    })
}

pub(super) fn list(database: &Path, before: Option<i64>) -> Result<FeedPage, FeedError> {
    let mut db = catalog::open_verified(database)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let mut items = tx
        .prepare(&format!(
            "SELECT {ITEM_COLUMNS} WHERE (?1 IS NULL OR i.sequence < ?1)
             ORDER BY i.sequence DESC LIMIT ?2"
        ))?
        .query_map(params![before, i64::from(FEED_PAGE) + 1], item)?
        .collect::<Result<Vec<_>, _>>()?;
    let next_before = if items.len() > usize::from(FEED_PAGE) {
        items.truncate(usize::from(FEED_PAGE));
        items.last().map(|last| last.sequence)
    } else {
        None
    };
    let (seen_through, unread) = read_state(&tx)?;
    let page = FeedPage {
        today: today(&tx)?,
        unread,
        seen_through,
        items,
        next_before,
    };
    tx.commit()?;
    Ok(page)
}

pub(super) fn see(database: &Path, through: u64) -> Result<FeedSeen, FeedError> {
    let mut db = catalog::open_verified(database)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let newest = newest(&tx)?;
    let through = i64::try_from(through)
        .ok()
        .filter(|through| *through <= newest)
        .ok_or(FeedError::AheadOfFeed { through, newest })?;
    tx.execute(
        "INSERT INTO host_feed_state(singleton, seen_through) VALUES (1, ?1)
         ON CONFLICT(singleton) DO UPDATE SET seen_through = max(seen_through, excluded.seen_through)",
        [through],
    )?;
    let (seen_through, unread) = read_state(&tx)?;
    tx.commit()?;
    Ok(FeedSeen {
        seen_through,
        unread,
    })
}

pub(super) fn dismiss(database: &Path, id: Uuid, now_ms: i64) -> Result<FeedItem, FeedError> {
    let mut db = catalog::open_verified(database)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !produce::done(&tx, id, now_ms)? {
        return Err(FeedError::NotFound(id));
    }
    let dismissed = tx.query_row(
        &format!("SELECT {ITEM_COLUMNS} WHERE i.id = ?1"),
        [id.to_string()],
        item,
    )?;
    tx.commit()?;
    Ok(dismissed)
}

/// `seen_through`, 0 until the owner first sees the feed, and the open items
/// above it.
fn read_state(db: &Connection) -> rusqlite::Result<(i64, u32)> {
    let seen_through: i64 = db
        .query_row(
            "SELECT seen_through FROM host_feed_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let unread = db.query_row(
        "SELECT count(*) FROM host_feed_items WHERE sequence > ?1 AND done_at_ms IS NULL",
        [seen_through],
        |row| row.get(0),
    )?;
    Ok((seen_through, unread))
}

/// The Today line: the stored sentence while no open item that needs the
/// owner is newer than it, and otherwise the Host's line from the count of
/// open items that need the owner, so the card never says quiet above one.
fn today(db: &Connection) -> rusqlite::Result<String> {
    let needing = item::needing_owner();
    let sentence = db
        .query_row(
            &format!(
                "SELECT t.sentence FROM host_today t WHERE t.singleton = 1
                 AND NOT EXISTS(SELECT 1 FROM host_feed_items i
                     WHERE i.kind IN ({needing}) AND i.done_at_ms IS NULL
                       AND i.sequence > t.through_sequence)"
            ),
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(sentence) = sentence {
        return Ok(sentence);
    }
    let open: u32 = db.query_row(
        &format!(
            "SELECT count(*) FROM host_feed_items WHERE kind IN ({needing}) AND done_at_ms IS NULL"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(match open {
        0 => "All quiet.".to_owned(),
        1 => "1 thing needs you.".to_owned(),
        open => format!("{open} things need you."),
    })
}
