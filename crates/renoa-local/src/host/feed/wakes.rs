//! Which items wake the owner's phone, and the record that one did.
//!
//! An open item whose urgency wakes is owed a wake until the node that serves
//! this Host has had the coordinator acknowledge one for it, and records that
//! here, once. An item answered or dismissed before then is owed nothing.

use std::path::Path;

use rusqlite::{Transaction, params};
use uuid::Uuid;

use super::{FeedError, Urgency, item};
use crate::host::catalog::{self, HostCatalogError};

/// The most owed wakes one read returns, oldest first.
const OWED_PAGE: i64 = 20;

/// An item the owner's phone has not yet been woken for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedWake {
    /// The item's id, which names the wake to the coordinator.
    pub id: Uuid,
    /// Whether the phone rings for it rather than buzzes.
    pub rings: bool,
}

/// Gives every item the time its wake was acknowledged: schema 49. A table
/// that has it already is left alone, so the reset's cutover can run it on
/// any catalog.
pub(in crate::host) fn add_wakes(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('host_feed_items')
            WHERE name = 'wake_sent_at_ms')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        tx.execute_batch("ALTER TABLE host_feed_items ADD COLUMN wake_sent_at_ms INTEGER;")?;
    }
    Ok(())
}

pub(super) fn owed(database: &Path) -> Result<Vec<FeedWake>, FeedError> {
    let db = catalog::open_verified(database)?;
    let mut statement = db.prepare(&format!(
        "SELECT id, urgency FROM host_feed_items
         WHERE urgency IN ({waking}) AND wake_sent_at_ms IS NULL AND done_at_ms IS NULL
         ORDER BY sequence LIMIT ?1",
        waking = item::waking(),
    ))?;
    let rows = statement.query_map([OWED_PAGE], |row| {
        let id: String = row.get(0)?;
        let urgency: Urgency = row.get(1)?;
        Ok((id, urgency))
    })?;
    let mut wakes = Vec::new();
    for row in rows {
        let (id, urgency) = row?;
        let id = Uuid::parse_str(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        wakes.push(FeedWake {
            id,
            rings: urgency.rings(),
        });
    }
    Ok(wakes)
}

/// Records that item `id`'s wake was acknowledged at `now_ms`, once. An item
/// removed meanwhile is nothing to record.
pub(super) fn sent(database: &Path, id: Uuid, now_ms: i64) -> Result<(), FeedError> {
    catalog::open_verified(database)?.execute(
        "UPDATE host_feed_items SET wake_sent_at_ms = ?2
         WHERE id = ?1 AND wake_sent_at_ms IS NULL",
        params![id.to_string(), now_ms],
    )?;
    Ok(())
}
