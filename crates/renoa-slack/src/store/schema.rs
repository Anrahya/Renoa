use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension as _, params};

use super::Binding;
use crate::SlackError;

pub(super) fn open(directory: &Path) -> Result<(File, Connection), SlackError> {
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".slack.lock"))?;
    lease.try_lock().map_err(|e| {
        SlackError::Invalid(format!(
            "Slack data directory is already owned or cannot be locked: {e}"
        ))
    })?;
    let path = directory.join("slack.sqlite3");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    drop(file);
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
    )?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => connection.execute_batch(SCHEMA)?,
        1 => connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE sessions ADD COLUMN agent_id TEXT; PRAGMA user_version=2; COMMIT;")?,
        2..=4 => {}
        _ => {
            return Err(SlackError::Invalid(format!(
                "unsupported Slack schema {version}"
            )));
        }
    }
    if version < 3 {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE bot_channels (
                agent_id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, channel_id TEXT UNIQUE,
                state TEXT NOT NULL CHECK(state IN('pending','creating','inviting','ready')),
                error TEXT,
                CHECK((state IN('pending','creating') AND channel_id IS NULL) OR
                      (state IN('inviting','ready') AND channel_id IS NOT NULL))
            ) STRICT;
            PRAGMA user_version=3; COMMIT;",
        )?;
    }
    if version < 4 {
        connection.execute_batch("BEGIN IMMEDIATE; ALTER TABLE requests ADD COLUMN surface_context TEXT; PRAGMA user_version=4; COMMIT;")?;
    }
    Ok((lease, connection))
}

pub(super) fn bind(connection: &mut Connection, binding: &Binding<'_>) -> Result<(), SlackError> {
    let transaction = connection.transaction()?;
    let existing = transaction.query_row(
        "SELECT host_id, agent_id, team, bot, allowed_user, workspace FROM identity WHERE singleton=1",
        [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, Vec<u8>>(5)?)),
    ).optional()?;
    let expected = (
        binding.host_id.to_string(),
        binding.agent_id.to_string(),
        binding.team.to_owned(),
        binding.bot.to_owned(),
        binding.user.to_owned(),
        binding.workspace.as_os_str().as_encoded_bytes().to_vec(),
    );
    if let Some(existing) = existing {
        if existing != expected {
            return Err(SlackError::Invalid(
                "Slack data belongs to a different Host, agent, workspace, bot, or operator"
                    .to_owned(),
            ));
        }
    } else {
        transaction.execute(
            "INSERT INTO identity VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                expected.0, expected.1, expected.2, expected.3, expected.4, expected.5
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

const SCHEMA: &str = "BEGIN IMMEDIATE;
CREATE TABLE identity (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1), host_id TEXT NOT NULL, agent_id TEXT NOT NULL,
 team TEXT NOT NULL, bot TEXT NOT NULL, allowed_user TEXT NOT NULL, workspace BLOB NOT NULL
) STRICT;
CREATE TABLE sessions (session_id TEXT PRIMARY KEY, channel TEXT NOT NULL, thread TEXT NOT NULL, agent_id TEXT) STRICT;
CREATE TABLE conversations (
 channel TEXT NOT NULL, thread TEXT NOT NULL, session_id TEXT NOT NULL REFERENCES sessions(session_id),
 PRIMARY KEY(channel, thread)
) STRICT;
CREATE TABLE requests (
 seq INTEGER PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, channel TEXT NOT NULL, thread TEXT NOT NULL,
 message_ts TEXT NOT NULL, command_json TEXT NOT NULL, executes_model INTEGER NOT NULL CHECK(executes_model IN(0,1)),
 session_id TEXT NOT NULL REFERENCES sessions(session_id), observed_at_ms INTEGER NOT NULL,
 state TEXT NOT NULL CHECK(state IN('queued','running','ready','done')),
 cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN(0,1)), cancel_target TEXT REFERENCES requests(request_id),
 reply_state TEXT NOT NULL DEFAULT 'pending' CHECK(reply_state IN('pending','sending','known','unknown','failed')),
 reply_ts TEXT, result TEXT, UNIQUE(channel, message_ts)
) STRICT;
CREATE TABLE messages (
 channel TEXT NOT NULL, message_ts TEXT NOT NULL, thread TEXT NOT NULL, input TEXT NOT NULL,
 request_id TEXT UNIQUE REFERENCES requests(request_id), PRIMARY KEY(channel,message_ts)
) STRICT;
CREATE TABLE receipts (
 event_id TEXT PRIMARY KEY, channel TEXT NOT NULL, message_ts TEXT NOT NULL,
 FOREIGN KEY(channel,message_ts) REFERENCES messages(channel,message_ts)
) STRICT;
CREATE TABLE deliveries (
 request_seq INTEGER NOT NULL REFERENCES requests(seq), chunk INTEGER NOT NULL CHECK(chunk>=0), text TEXT NOT NULL,
 slack_ts TEXT, state TEXT NOT NULL CHECK(state IN('pending','sending','sent','unknown','failed')), error TEXT,
 PRIMARY KEY(request_seq,chunk)
) STRICT;
CREATE INDEX work_queue ON requests(seq) WHERE state IN('queued','ready');
PRAGMA user_version=2;
COMMIT;";
