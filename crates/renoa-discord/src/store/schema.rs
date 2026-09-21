use std::{fs::OpenOptions, path::Path, time::SystemTime};

use rusqlite::{Connection, TransactionBehavior};

use crate::DiscordError;

pub(super) const DATABASE_FILE: &str = "discord.sqlite3";
const LEASE_FILE: &str = ".discord.lock";
const SCHEMA_VERSION: i64 = 3;

pub(super) fn open(path: &Path) -> Result<Connection, DiscordError> {
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
        2 => migrate_identity_binding(&connection)?,
        SCHEMA_VERSION => {}
        other => {
            return Err(DiscordError::Invalid(format!(
                "Discord surface database schema {other} is not supported schema {SCHEMA_VERSION}"
            )));
        }
    }
    Ok(connection)
}

pub(super) fn acquire_lease(directory: &Path) -> Result<std::fs::File, DiscordError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(LEASE_FILE))?;
    restrict_file(&file)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => DiscordError::Invalid(
            "another Renoa Discord surface already owns this data directory".to_owned(),
        ),
        std::fs::TryLockError::Error(error) => DiscordError::Io(error),
    })?;
    Ok(file)
}

pub(super) fn immediate_transaction(
    connection: &mut Connection,
) -> Result<rusqlite::Transaction<'_>, DiscordError> {
    Ok(connection.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

pub(super) fn now_ms() -> Result<i64, DiscordError> {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| DiscordError::Invalid("system clock is before the unix epoch".to_owned()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| DiscordError::Invalid("system clock is out of range".to_owned()))
}

fn initialize(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(&format!(
        "CREATE TABLE identity (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            guild_id TEXT NOT NULL CHECK (
                length(guild_id) BETWEEN 1 AND 20
                AND guild_id NOT GLOB '*[^0-9]*'
                AND guild_id NOT GLOB '0*'
            ),
            operator_user_id TEXT NOT NULL CHECK (
                length(operator_user_id) BETWEEN 1 AND 20
                AND operator_user_id NOT GLOB '*[^0-9]*'
                AND operator_user_id NOT GLOB '0*'
            ),
            agent_id TEXT NOT NULL CHECK (length(agent_id) = 36),
            bot_user_id TEXT CHECK (
                bot_user_id IS NULL
                OR (
                    length(bot_user_id) BETWEEN 1 AND 20
                    AND bot_user_id NOT GLOB '*[^0-9]*'
                    AND bot_user_id NOT GLOB '0*'
                )
            )
         ) STRICT;

         CREATE TABLE messages (
            message_id TEXT PRIMARY KEY CHECK (
                length(message_id) BETWEEN 1 AND 20
                AND message_id NOT GLOB '*[^0-9]*'
                AND message_id NOT GLOB '0*'
            ),
            channel_id TEXT NOT NULL CHECK (
                length(channel_id) BETWEEN 1 AND 20
                AND channel_id NOT GLOB '*[^0-9]*'
                AND channel_id NOT GLOB '0*'
            ),
            author_id TEXT NOT NULL CHECK (
                length(author_id) BETWEEN 1 AND 20
                AND author_id NOT GLOB '*[^0-9]*'
                AND author_id NOT GLOB '0*'
            ),
            canonical BLOB NOT NULL CHECK (length(canonical) > 0),
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
         ) STRICT;

         {CONVERSATION_SCHEMA}",
    ))?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_identity_binding(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(&format!(
        "ALTER TABLE identity ADD COLUMN bot_user_id TEXT;
         {CONVERSATION_SCHEMA}"
    ))?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

const CONVERSATION_SCHEMA: &str = "
CREATE TABLE conversations (
    channel_id TEXT PRIMARY KEY CHECK (
        length(channel_id) BETWEEN 1 AND 20
        AND channel_id NOT GLOB '*[^0-9]*'
        AND channel_id NOT GLOB '0*'
    ),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36)
) STRICT;

CREATE TABLE turns (
    message_id TEXT PRIMARY KEY REFERENCES messages(message_id),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36),
    request_id TEXT NOT NULL CHECK (length(request_id) = 36),
    prompt TEXT NOT NULL,
    result TEXT,
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'ready')),
    CHECK (
        (state IN ('queued', 'running') AND result IS NULL)
        OR (state = 'ready' AND result IS NOT NULL)
    )
) STRICT;

CREATE TABLE deliveries (
    message_id TEXT NOT NULL REFERENCES turns(message_id),
    chunk INTEGER NOT NULL CHECK (chunk >= 0),
    body TEXT NOT NULL CHECK (length(body) > 0),
    state TEXT NOT NULL CHECK (state IN ('pending', 'sending', 'sent', 'unknown', 'failed')),
    reply_id TEXT,
    PRIMARY KEY (message_id, chunk),
    CHECK (
        (state = 'sent' AND reply_id IS NOT NULL)
        OR (state <> 'sent' AND reply_id IS NULL)
    )
) STRICT;

CREATE TABLE gateway (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    session_id TEXT,
    resume_url TEXT,
    sequence INTEGER,
    CHECK (
        (session_id IS NULL AND resume_url IS NULL)
        OR (length(session_id) > 0 AND length(resume_url) > 0)
    )
) STRICT;

CREATE INDEX turns_ready ON turns(message_id) WHERE state = 'ready';
CREATE INDEX turns_queued ON turns(message_id) WHERE state = 'queued';
";

pub(super) fn restrict_database(path: &Path) -> Result<(), DiscordError> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    restrict_file(&file)?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn restrict_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(super) fn restrict_directory(_path: &Path) -> Result<(), std::io::Error> {
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
