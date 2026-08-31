use std::{fs::OpenOptions, path::Path};

use rusqlite::{Connection, TransactionBehavior};

use super::StoreError;

pub(super) const DATABASE_FILE: &str = "telegram.sqlite3";
pub(super) const LEASE_FILE: &str = ".telegram.lock";
const SCHEMA_VERSION: i64 = 1;

pub(super) fn open(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;",
    )?;
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    match version {
        0 => initialize(&connection)?,
        SCHEMA_VERSION => {}
        newer if newer > SCHEMA_VERSION => {
            return Err(StoreError::Invalid(format!(
                "Telegram surface database schema {newer} is newer than supported {SCHEMA_VERSION}"
            )));
        }
        older => {
            return Err(StoreError::Invalid(format!(
                "Telegram surface database schema {older} has no migration to {SCHEMA_VERSION}"
            )));
        }
    }
    Ok(connection)
}

pub(super) fn acquire_lease(directory: &Path) -> Result<std::fs::File, StoreError> {
    let path = directory.join(LEASE_FILE);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    restrict_file(&file)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => StoreError::Invalid(
            "another Renoa Telegram surface already owns this data directory".to_owned(),
        ),
        std::fs::TryLockError::Error(error) => StoreError::Io(error),
    })?;
    Ok(file)
}

pub(super) fn restrict_database(path: &Path) -> Result<(), StoreError> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    restrict_file(&file)?;
    Ok(())
}

fn initialize(connection: &Connection) -> Result<(), StoreError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "CREATE TABLE surface_identity (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            bot_id INTEGER NOT NULL CHECK (bot_id > 0),
            allowed_user_id INTEGER NOT NULL CHECK (allowed_user_id > 0),
            workspace BLOB NOT NULL CHECK (length(workspace) > 0),
            next_update_id INTEGER NOT NULL CHECK (next_update_id >= 0)
         ) STRICT;

         CREATE TABLE surface_sessions (
            session_id TEXT PRIMARY KEY CHECK (length(session_id) = 36),
            chat_id INTEGER NOT NULL,
            thread_id INTEGER NOT NULL CHECK (thread_id >= 0),
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
         ) STRICT;

         CREATE TABLE conversations (
            chat_id INTEGER NOT NULL,
            thread_id INTEGER NOT NULL CHECK (thread_id >= 0),
            session_id TEXT NOT NULL UNIQUE CHECK (length(session_id) = 36),
            updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
            PRIMARY KEY (chat_id, thread_id),
            FOREIGN KEY (session_id) REFERENCES surface_sessions(session_id)
         ) STRICT;

         CREATE TABLE updates (
            update_id INTEGER PRIMARY KEY CHECK (update_id >= 0),
            canonical_json BLOB NOT NULL CHECK (length(canonical_json) > 0),
            chat_id INTEGER,
            thread_id INTEGER CHECK (thread_id IS NULL OR thread_id >= 0),
            message_id INTEGER,
            session_id TEXT CHECK (session_id IS NULL OR length(session_id) = 36),
            request_id TEXT NOT NULL CHECK (length(request_id) = 36),
            draft_id INTEGER NOT NULL CHECK (draft_id != 0),
            kind TEXT NOT NULL CHECK (kind IN (
                'prompt', 'compact', 'new', 'status', 'cancel', 'notice', 'stopped', 'ignored'
            )),
            payload TEXT,
            incoming_draft_id INTEGER,
            state TEXT NOT NULL CHECK (state IN (
                'queued', 'running', 'ready', 'delivering', 'delivered',
                'delivery_unknown', 'delivery_failed', 'ignored'
            )),
            cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
            result TEXT,
            delivery_cursor INTEGER NOT NULL DEFAULT 0 CHECK (delivery_cursor >= 0),
            error TEXT,
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
            updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
            FOREIGN KEY (session_id) REFERENCES surface_sessions(session_id)
         ) STRICT;

         CREATE TABLE delivery_messages (
            update_id INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
            telegram_message_id INTEGER NOT NULL,
            delivered_at_ms INTEGER NOT NULL CHECK (delivered_at_ms >= 0),
            PRIMARY KEY (update_id, chunk_index),
            FOREIGN KEY (update_id) REFERENCES updates(update_id) ON DELETE CASCADE
         ) STRICT;

         CREATE INDEX updates_work_queue
         ON updates(update_id)
         WHERE state IN ('queued', 'ready');

         PRAGMA user_version = 1;",
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(unix)]
fn restrict_file(file: &std::fs::File) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file(_file: &std::fs::File) -> Result<(), std::io::Error> {
    Ok(())
}

pub(super) fn immediate_transaction(
    connection: &mut Connection,
) -> Result<rusqlite::Transaction<'_>, StoreError> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}
