//! "While you were away": what happened on the Host that its owner may want to
//! see, newest first, with one action on each item, and the Today line above it.
//!
//! Every item is born in the transaction that records the fact it reports, so
//! a crash leaves both or neither and a rejected change leaves nothing. Each
//! item's identity derives from its kind and its source, and producers insert
//! with `INSERT OR IGNORE`, so a replay converges on one item. The owner's read
//! state is one number, `seen_through`, that only rises, so every device and
//! every retry agrees on it.

use std::path::{Path, PathBuf};

use rusqlite::Transaction;
use uuid::Uuid;

use super::catalog::{self, HostCatalogError};

mod item;
mod produce;
mod read;
pub(super) mod retention;
#[cfg(test)]
mod tests;
mod wakes;

pub(crate) use item::{BODY_CHARS, CHOICE_CHARS, CHOICES, TODAY_CHARS};
pub use item::{FeedAction, FeedKind, Urgency};
pub(crate) use produce::{
    Ability, ForOwner, ability_turned_on, owner_asked, owner_told, today_set,
};
pub(super) use produce::{RunEnded, run_ended};
pub use read::{FeedAgent, FeedItem, FeedPage, FeedSeen};
pub use wakes::FeedWake;
pub(super) use wakes::add_wakes;

/// Items one page of [`HostFeed::list`] holds.
pub const FEED_PAGE: u8 = 50;

/// Creates the feed's three tables, empty. Fresh catalogs, the schema 46
/// upgrade and the reset's cutover share it, so it is idempotent.
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    tx.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS host_feed_items (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ({kinds})),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND {title}),
            body TEXT NOT NULL CHECK (length(body) <= {body}),
            action TEXT NOT NULL CHECK (action IN ({actions})),
            subject_id TEXT NOT NULL CHECK (length(subject_id) > 0),
            created_at_ms INTEGER NOT NULL,
            done_at_ms INTEGER
        ) STRICT;
        CREATE INDEX IF NOT EXISTS host_feed_items_by_subject ON host_feed_items(subject_id, kind);
        CREATE TABLE IF NOT EXISTS host_feed_state (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            seen_through INTEGER NOT NULL CHECK (seen_through >= 0)
        ) STRICT;
        CREATE TABLE IF NOT EXISTS host_today (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            sentence TEXT NOT NULL CHECK (
                length(sentence) BETWEEN 1 AND {sentence}
                AND instr(sentence, char(10)) = 0
                AND instr(sentence, char(13)) = 0
            ),
            through_sequence INTEGER NOT NULL CHECK (through_sequence >= 0)
        ) STRICT;",
        kinds = item::sql_list(FeedKind::ALL.map(FeedKind::as_str)),
        actions = item::sql_list(FeedAction::ALL.map(FeedAction::as_str)),
        title = item::TITLE_CHARS,
        body = item::BODY_CHARS,
        sentence = item::TODAY_CHARS,
    ))?;
    Ok(())
}

/// Gives every item an urgency and an ask its choices: schema 48. Existing
/// items keep `whenever`, so an upgrade wakes no phone for them. A table that
/// has them already is left alone, so the reset's cutover can run it on any
/// catalog.
pub(super) fn add_urgency(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('host_feed_items') WHERE name = 'urgency')",
        [],
        |row| row.get(0),
    )?;
    if present {
        return Ok(());
    }
    tx.execute_batch(&format!(
        "ALTER TABLE host_feed_items ADD COLUMN urgency TEXT NOT NULL DEFAULT 'whenever'
            CHECK (urgency IN ({urgencies}));
         ALTER TABLE host_feed_items ADD COLUMN choices TEXT
            CHECK (choices IS NULL OR json_valid(choices));",
        urgencies = item::sql_list(Urgency::ALL.map(Urgency::as_str)),
    ))?;
    Ok(())
}

/// Marks done the open asks a message on `session` answers; see
/// [`produce::asks_answered`].
///
/// # Errors
/// Returns catalog and stored-data failures.
pub(crate) fn answer_asks(
    database: &Path,
    session: renoa_kernel::SessionId,
    command: renoa_kernel::CommandId,
    said_at_ms: i64,
) -> Result<usize, FeedError> {
    let mut db = catalog::open_verified(database)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let answered = produce::asks_answered(&tx, session, command, said_at_ms)?;
    tx.commit()?;
    Ok(answered)
}

/// The newest sequence the feed has handed out, pruned or not; 0 before the
/// first item.
pub(super) fn newest(db: &rusqlite::Connection) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'host_feed_items'), 0)",
        [],
        |row| row.get(0),
    )
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FeedError {
    #[error("no feed item has id {0}")]
    NotFound(Uuid),
    #[error(
        "seen_through {through} is past the newest feed item, {newest}; send a sequence the feed has listed"
    )]
    AheadOfFeed { through: u64, newest: i64 },
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error("feed storage task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

/// The owner's reading of the feed, opened on a Host's data root without
/// constructing an execution Host. The relay authenticates the owner; nothing
/// here takes an identity from a request.
#[derive(Clone)]
pub struct HostFeed {
    database: PathBuf,
}

impl HostFeed {
    /// Opens the feed of the Host stored under `root`.
    ///
    /// # Errors
    /// Returns a missing or incompatible catalog.
    pub fn open(root: &Path) -> Result<Self, FeedError> {
        let database = root.join(catalog::HOST_DATABASE);
        catalog::open_verified(&database)?;
        Ok(Self { database })
    }

    /// One page of items, newest first: the newest [`FEED_PAGE`], or those
    /// below `before`, with the Today line, the unread count and the owner's
    /// `seen_through`.
    ///
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn list(&self, before: Option<i64>) -> Result<FeedPage, FeedError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || read::list(&database, before)).await?
    }

    /// Raises `seen_through` to `through` when it is higher, and returns the
    /// owner's read state.
    ///
    /// # Errors
    /// Rejects a sequence above the newest item, naming both.
    pub async fn see(&self, through: u64) -> Result<FeedSeen, FeedError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || read::see(&database, through)).await?
    }

    /// Marks item `id` done at `now_ms`, once; repeating it changes nothing.
    ///
    /// # Errors
    /// Rejects an unknown id, naming it.
    pub async fn dismiss(&self, id: Uuid, now_ms: i64) -> Result<FeedItem, FeedError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || read::dismiss(&database, id, now_ms)).await?
    }

    /// The open items whose urgency wakes the owner's phone and whose wake
    /// was not yet acknowledged, oldest first, a page at a time.
    ///
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn owed_wakes(&self) -> Result<Vec<FeedWake>, FeedError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || wakes::owed(&database)).await?
    }

    /// Records that the coordinator acknowledged item `id`'s wake at
    /// `now_ms`; repeating it changes nothing.
    ///
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn wake_sent(&self, id: Uuid, now_ms: i64) -> Result<(), FeedError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || wakes::sent(&database, id, now_ms)).await?
    }
}
