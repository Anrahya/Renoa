use std::{fs::OpenOptions, path::Path};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};

use super::StoreError;

pub(super) const DATABASE_FILE: &str = "telegram.sqlite3";
pub(super) const LEASE_FILE: &str = ".telegram.lock";
const SCHEMA_VERSION: i64 = 3;

const UPDATE_COLUMNS: &str = "(
    update_id INTEGER PRIMARY KEY CHECK (update_id >= 0),
    canonical_json BLOB NOT NULL CHECK (length(canonical_json) > 0),
    chat_id INTEGER,
    thread_id INTEGER CHECK (thread_id IS NULL OR thread_id >= 0),
    message_id INTEGER,
    session_id TEXT CHECK (session_id IS NULL OR length(session_id) = 36),
    request_id TEXT NOT NULL CHECK (length(request_id) = 36),
    draft_id INTEGER NOT NULL CHECK (draft_id != 0),
    kind TEXT NOT NULL CHECK (kind IN (
        'prompt', 'compact', 'new', 'status', 'model', 'reasoning', 'cancel',
        'notice', 'stopped', 'ignored'
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
) STRICT;";

const ACTION_SCHEMA: &str = "
    CREATE TABLE surface_actions (
        action_id TEXT PRIMARY KEY CHECK (length(action_id) BETWEEN 1 AND 1024),
        update_id INTEGER NOT NULL REFERENCES updates(update_id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (kind = 'open_url'),
        title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 256),
        message TEXT NOT NULL CHECK (length(message) BETWEEN 1 AND 2048),
        button TEXT NOT NULL CHECK (length(button) BETWEEN 1 AND 64),
        url TEXT NOT NULL CHECK (length(url) BETWEEN 1 AND 16384),
        expires_at_ms INTEGER CHECK (expires_at_ms IS NULL OR expires_at_ms > 0),
        state TEXT NOT NULL CHECK (state IN (
            'pending', 'delivering', 'delivered', 'delivery_unknown', 'delivery_failed'
        )),
        telegram_message_id INTEGER,
        error TEXT,
        created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
        updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
        CHECK (
            (state IN ('pending', 'delivering')
             AND telegram_message_id IS NULL AND error IS NULL)
            OR
            (state = 'delivered' AND telegram_message_id IS NOT NULL AND error IS NULL)
            OR
            (state IN ('delivery_unknown', 'delivery_failed')
             AND telegram_message_id IS NULL AND length(error) > 0)
        )
    ) STRICT;
";

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
        1 => {
            migrate_v1_to_v2(&connection)?;
            migrate_v2_to_v3(&connection)?;
        }
        2 => migrate_v2_to_v3(&connection)?,
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

         ",
    )?;
    transaction.execute_batch(&format!("CREATE TABLE updates {UPDATE_COLUMNS}"))?;
    transaction.execute_batch(
        "CREATE TABLE delivery_messages (
            update_id INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
            telegram_message_id INTEGER NOT NULL,
            delivered_at_ms INTEGER NOT NULL CHECK (delivered_at_ms >= 0),
            PRIMARY KEY (update_id, chunk_index),
            FOREIGN KEY (update_id) REFERENCES updates(update_id) ON DELETE CASCADE
         ) STRICT;

         CREATE INDEX updates_work_queue
         ON updates(update_id)
         WHERE state IN ('queued', 'ready');",
    )?;
    transaction.execute_batch(ACTION_SCHEMA)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v1_to_v2(connection: &Connection) -> Result<(), StoreError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(ACTION_SCHEMA)?;
    transaction.pragma_update(None, "user_version", 2)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v2_to_v3(connection: &Connection) -> Result<(), StoreError> {
    connection.pragma_update(None, "foreign_keys", false)?;
    let migration = (|| {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(&format!("CREATE TABLE updates_v3 {UPDATE_COLUMNS}"))?;
        transaction.execute_batch(
            "INSERT INTO updates_v3(
                update_id, canonical_json, chat_id, thread_id, message_id, session_id,
                request_id, draft_id, kind, payload, incoming_draft_id, state,
                cancel_requested, result, delivery_cursor, error, created_at_ms, updated_at_ms
             )
             SELECT
                update_id, canonical_json, chat_id, thread_id, message_id, session_id,
                request_id, draft_id, kind, payload, incoming_draft_id, state,
                cancel_requested, result, delivery_cursor, error, created_at_ms, updated_at_ms
             FROM updates;
             DROP TABLE updates;
             ALTER TABLE updates_v3 RENAME TO updates;
             CREATE INDEX updates_work_queue
             ON updates(update_id)
             WHERE state IN ('queued', 'ready');",
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok::<(), rusqlite::Error>(())
    })();
    let foreign_keys = connection.pragma_update(None, "foreign_keys", true);
    migration?;
    foreign_keys?;
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?;
    if violation.is_some() {
        return Err(StoreError::Invalid(
            "Telegram surface database migration violated a foreign key".to_owned(),
        ));
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::{Connection, OptionalExtension as _};
    use tempfile::tempdir;

    use super::{ACTION_SCHEMA, SCHEMA_VERSION, UPDATE_COLUMNS, open};

    #[test]
    fn version_two_migration_preserves_work_deliveries_and_actions() {
        let directory = tempdir().expect("temporary schema fixture");
        let database = directory.path().join("telegram.sqlite3");
        create_version_two_fixture(&database);

        let migrated = open(&database).expect("migrate version two database");
        assert_migrated_state(&migrated);
    }

    fn create_version_two_fixture(database: &Path) {
        let connection = Connection::open(database).expect("open version two fixture");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        let old_columns = UPDATE_COLUMNS.replace(
            "'status', 'model', 'reasoning', 'cancel',",
            "'status', 'cancel',",
        );
        connection
            .execute_batch(&format!(
                "CREATE TABLE surface_sessions (
                    session_id TEXT PRIMARY KEY CHECK (length(session_id) = 36),
                    chat_id INTEGER NOT NULL,
                    thread_id INTEGER NOT NULL CHECK (thread_id >= 0),
                    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
                 ) STRICT;
                 CREATE TABLE updates {old_columns}
                 CREATE TABLE delivery_messages (
                    update_id INTEGER NOT NULL,
                    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
                    telegram_message_id INTEGER NOT NULL,
                    delivered_at_ms INTEGER NOT NULL CHECK (delivered_at_ms >= 0),
                    PRIMARY KEY (update_id, chunk_index),
                    FOREIGN KEY (update_id) REFERENCES updates(update_id) ON DELETE CASCADE
                 ) STRICT;
                 CREATE INDEX updates_work_queue ON updates(update_id)
                 WHERE state IN ('queued', 'ready');"
            ))
            .expect("create version two tables");
        connection
            .execute_batch(ACTION_SCHEMA)
            .expect("create version two action table");
        connection
            .execute_batch(
                "INSERT INTO surface_sessions VALUES (
                    '00000000-0000-0000-0000-000000000001', 42, 0, 1
                 );
                 INSERT INTO updates(
                    update_id, canonical_json, chat_id, thread_id, message_id, session_id,
                    request_id, draft_id, kind, payload, state, created_at_ms, updated_at_ms
                 ) VALUES (
                    1, x'01', 42, 0, 7, '00000000-0000-0000-0000-000000000001',
                    '00000000-0000-0000-0000-000000000002', 8, 'prompt', 'hello',
                    'delivered', 1, 1
                 );
                 INSERT INTO delivery_messages VALUES (1, 0, 9, 1);
                 INSERT INTO surface_actions(
                    action_id, update_id, kind, title, message, button, url, state,
                    telegram_message_id, created_at_ms, updated_at_ms
                 ) VALUES (
                    'action', 1, 'open_url', 'Authorize', 'Open it', 'Open',
                    'https://provider.example/authorize', 'delivered', 10, 1, 1
                 );
                 PRAGMA user_version = 2;",
            )
            .expect("populate version two fixture");
        assert!(
            connection
                .execute(
                    "INSERT INTO updates(
                        update_id, canonical_json, request_id, draft_id, kind, state,
                        created_at_ms, updated_at_ms
                     ) VALUES (
                        2, x'02', '00000000-0000-0000-0000-000000000003', 9,
                        'model', 'queued', 1, 1
                     )",
                    [],
                )
                .is_err()
        );
    }

    fn assert_migrated_state(migrated: &Connection) {
        let version = migrated
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("read migrated version");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(
            migrated
                .query_row("SELECT count(*) FROM delivery_messages", [], |row| row
                    .get::<_, i64>(0))
                .expect("count preserved deliveries"),
            1
        );
        assert_eq!(
            migrated
                .query_row("SELECT count(*) FROM surface_actions", [], |row| row
                    .get::<_, i64>(0))
                .expect("count preserved actions"),
            1
        );
        assert!(
            migrated
                .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
                .optional()
                .expect("check migrated foreign keys")
                .is_none()
        );
        migrated
            .execute(
                "INSERT INTO updates(
                    update_id, canonical_json, request_id, draft_id, kind, payload, state,
                    created_at_ms, updated_at_ms
                 ) VALUES (
                    2, x'02', '00000000-0000-0000-0000-000000000003', 9,
                    'model', 'glm-5.3-flash', 'queued', 1, 1
                 )",
                [],
            )
            .expect("new model command is accepted after migration");
    }
}
