use std::{fs::OpenOptions, path::Path, time::SystemTime};

use rusqlite::{Connection, TransactionBehavior};

use crate::DiscordError;

pub(super) const DATABASE_FILE: &str = "discord.sqlite3";
const LEASE_FILE: &str = ".discord.lock";
const SCHEMA_VERSION: i64 = 7;

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
        1 => {
            migrate_v1(&connection)?;
            migrate_v3(&connection)?;
            migrate_v4(&connection)?;
            migrate_v5(&connection)?;
            migrate_v6(&connection)?;
        }
        3 => {
            migrate_v3(&connection)?;
            migrate_v4(&connection)?;
            migrate_v5(&connection)?;
            migrate_v6(&connection)?;
        }
        4 => {
            migrate_v4(&connection)?;
            migrate_v5(&connection)?;
            migrate_v6(&connection)?;
        }
        5 => {
            migrate_v5(&connection)?;
            migrate_v6(&connection)?;
        }
        6 => migrate_v6(&connection)?,
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

         {conversations}
         {progress}
         {places}
         {AUTHOR_SCHEMA}
         {GATEWAY_SCHEMA}
         {ACTION_SCHEMA}
         {CONTROL_SCHEMA}",
        conversations = conversation_schema(),
        progress = progress_schema(),
        places = places_schema(),
    ))?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

const SNOWFLAKE: &str =
    "length({0}) BETWEEN 1 AND 20 AND {0} NOT GLOB '*[^0-9]*' AND {0} NOT GLOB '0*'";

/// Every Discord conversation is one RCP task on its channel's agent. The
/// surface keeps only what it needs to submit messages and post replies; the
/// conversation itself lives in the coordinator's task journal.
fn conversation_schema() -> String {
    let channel = SNOWFLAKE.replace("{0}", "channel_id");
    format!(
        "CREATE TABLE tasks (
            task_id TEXT PRIMARY KEY CHECK (length(task_id) = 36),
            channel_id TEXT NOT NULL CHECK ({channel}),
            agent_id TEXT NOT NULL CHECK (length(agent_id) = 36),
            opened INTEGER NOT NULL DEFAULT 0 CHECK (opened IN (0, 1)),
            cursor INTEGER CHECK (cursor IS NULL OR cursor >= 0),
            current INTEGER NOT NULL CHECK (current IN (0, 1))
         ) STRICT;
         CREATE UNIQUE INDEX tasks_current ON tasks(channel_id) WHERE current = 1;

         CREATE TABLE turns (
            message_id TEXT PRIMARY KEY REFERENCES messages(message_id),
            task_id TEXT NOT NULL REFERENCES tasks(task_id),
            command_id TEXT NOT NULL UNIQUE CHECK (length(command_id) = 36),
            prompt TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('queued', 'submitted', 'answered'))
         ) STRICT;
         CREATE INDEX turns_queued ON turns(message_id) WHERE state = 'queued';

         CREATE TABLE replies (
            command_id TEXT PRIMARY KEY CHECK (length(command_id) = 36),
            task_id TEXT NOT NULL REFERENCES tasks(task_id),
            heading TEXT,
            answer TEXT,
            finished INTEGER NOT NULL DEFAULT 0 CHECK (finished IN (0, 1))
         ) STRICT;

         CREATE TABLE deliveries (
            command_id TEXT NOT NULL CHECK (length(command_id) = 36),
            chunk INTEGER NOT NULL CHECK (chunk >= 0),
            channel_id TEXT NOT NULL CHECK ({channel}),
            reply_to TEXT,
            body TEXT NOT NULL CHECK (length(body) > 0),
            state TEXT NOT NULL CHECK (state IN ('pending', 'sending', 'sent', 'unknown', 'failed')),
            reply_id TEXT,
            PRIMARY KEY (command_id, chunk),
            CHECK (
                (state = 'sent' AND reply_id IS NOT NULL)
                OR (state <> 'sent' AND reply_id IS NULL)
            )
         ) STRICT;"
    )
}

/// Posted progress messages not yet deleted, one per running command.
fn progress_schema() -> String {
    let channel = SNOWFLAKE.replace("{0}", "channel_id");
    let message = SNOWFLAKE.replace("{0}", "message_id");
    format!(
        "CREATE TABLE progress_messages (
            command_id TEXT PRIMARY KEY CHECK (length(command_id) = 36),
            channel_id TEXT NOT NULL CHECK ({channel}),
            message_id TEXT NOT NULL CHECK ({message})
         ) STRICT;"
    )
}

/// The server's channels and threads as gateway events describe them, and the
/// description of its place that each queued message is submitted with.
fn places_schema() -> String {
    let channel = SNOWFLAKE.replace("{0}", "channel_id");
    let parent = SNOWFLAKE.replace("{0}", "thread_parent_id");
    format!(
        "CREATE TABLE channels (
            channel_id TEXT PRIMARY KEY CHECK ({channel}),
            name TEXT,
            thread_parent_id TEXT CHECK (thread_parent_id IS NULL OR ({parent}))
         ) STRICT;
         ALTER TABLE turns ADD COLUMN context TEXT;"
    )
}

/// Whether each queued message is the operator's or a guest's.
const AUTHOR_SCHEMA: &str = "ALTER TABLE turns ADD COLUMN author TEXT NOT NULL
    DEFAULT 'principal' CHECK (author IN ('principal', 'guest'));";

const GATEWAY_SCHEMA: &str = "
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
";

/// Operator setup-link deliveries, keyed by the RCP command whose tool asked
/// for them. Only a digest of each link is stored.
const ACTION_SCHEMA: &str = "
CREATE TABLE actions (
    command_id TEXT NOT NULL CHECK (length(command_id) = 36),
    call_id TEXT NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN ('authorization', 'credentials')),
    digest BLOB NOT NULL CHECK (length(digest) = 32),
    state TEXT NOT NULL CHECK (state IN ('sending', 'sent', 'unknown', 'failed')),
    PRIMARY KEY (command_id, call_id, stage)
) STRICT;
";

const CONTROL_SCHEMA: &str = "
CREATE TABLE channel_bindings (
    channel_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL CHECK (length(agent_id) = 36),
    channel_name TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0)
) STRICT;
CREATE TABLE binding_receipts (
    operation_id TEXT PRIMARY KEY,
    request TEXT NOT NULL,
    result TEXT NOT NULL
) STRICT;";

/// Schema 1 lacked per-channel agents; it gains them on the way to schema 3.
fn migrate_v1(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch("ALTER TABLE conversations ADD COLUMN agent_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000' CHECK (length(agent_id) = 36);
        ALTER TABLE turns ADD COLUMN agent_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000' CHECK (length(agent_id) = 36);
        UPDATE conversations SET agent_id = (SELECT agent_id FROM identity WHERE singleton = 1);
        UPDATE turns SET agent_id = (SELECT agent_id FROM identity WHERE singleton = 1);")?;
    transaction.execute_batch(CONTROL_SCHEMA)?;
    transaction.pragma_update(None, "user_version", 3)?;
    transaction.commit()?;
    Ok(())
}

/// Schema 4 moves conversations into RCP tasks. In-process Host sessions and
/// their queued turns, replies, and setup deliveries do not carry over; the
/// connection identity, channel bindings, gateway cursor, and message
/// deduplication do.
fn migrate_v3(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(
        "DROP TABLE IF EXISTS actions;
         DROP TABLE deliveries;
         DROP TABLE turns;
         DROP TABLE conversations;",
    )?;
    transaction.execute_batch(&conversation_schema())?;
    transaction.execute_batch(ACTION_SCHEMA)?;
    transaction.pragma_update(None, "user_version", 4)?;
    transaction.commit()?;
    Ok(())
}

/// Schema 5 records posted progress messages, so a restart still deletes them.
fn migrate_v4(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(&progress_schema())?;
    transaction.pragma_update(None, "user_version", 5)?;
    transaction.commit()?;
    Ok(())
}

/// Schema 6 keeps the channel directory and each turn's surface context.
/// Messages already queued keep no context; the directory fills from the
/// gateway and from lookups as messages arrive.
fn migrate_v5(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(&places_schema())?;
    transaction.pragma_update(None, "user_version", 6)?;
    transaction.commit()?;
    Ok(())
}

/// Schema 7 records whether each message is the operator's or a guest's. A
/// message still queued is classified from its author; one already submitted
/// keeps `principal`, as it was sent, so its retry repeats it exactly.
fn migrate_v6(connection: &Connection) -> Result<(), DiscordError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(AUTHOR_SCHEMA)?;
    transaction.execute(
        "UPDATE turns SET author = 'guest'
         WHERE state = 'queued'
           AND (SELECT author_id FROM messages WHERE messages.message_id = turns.message_id)
               IS NOT (SELECT operator_user_id FROM identity WHERE singleton = 1)",
        [],
    )?;
    transaction.pragma_update(None, "user_version", 7)?;
    transaction.commit()?;
    Ok(())
}

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
