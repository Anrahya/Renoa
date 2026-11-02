//! What the note pass keeps: each main-agent conversation's activity and
//! unreviewed range, and each pass, frozen before its command is admitted and
//! settled once with the outcome its command ended with.

use std::path::{Path, PathBuf};

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

use super::{Kind, settle::Outcome};
use crate::host::catalog::HostCatalogError;

/// One row per conversation of the main agent with a person. `user_chars`
/// counts the person's text since the range began, once per command;
/// `check_at_ms` is when to ask `pass::due` again, `NULL` until new activity.
/// One row per pass; a pass without an outcome keeps its frozen message,
/// which the outcome clears, and a conversation has at most one. A completed
/// pass outlives its conversation, so deleting one never resets the person's
/// cap or gap.
pub(crate) const TABLES: &str = "CREATE TABLE IF NOT EXISTS host_memory_conversations (
        session_id TEXT PRIMARY KEY,
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        principal_id TEXT NOT NULL,
        workspace TEXT NOT NULL,
        last_activity_ms INTEGER NOT NULL,
        user_chars INTEGER NOT NULL CHECK (user_chars >= 0),
        counted_command TEXT,
        range_since_ms INTEGER,
        range_since_note INTEGER NOT NULL CHECK (range_since_note >= 0),
        check_at_ms INTEGER
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_memory_passes (
        command_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        principal_id TEXT NOT NULL,
        run_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('daytime', 'sweep')),
        pass_day TEXT NOT NULL,
        frozen_at_ms INTEGER NOT NULL,
        user_chars INTEGER NOT NULL CHECK (user_chars >= 0),
        prompt TEXT,
        outcome TEXT CHECK (outcome IN ('completed', 'cancelled', 'failed')),
        noted INTEGER CHECK (noted >= 0),
        finished_at_ms INTEGER,
        CHECK ((prompt IS NULL) = (outcome IS NOT NULL)),
        CHECK ((outcome IS NULL) = (noted IS NULL)),
        CHECK ((outcome IS NULL) = (finished_at_ms IS NULL))
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS host_memory_passes_unsettled
        ON host_memory_passes(session_id) WHERE outcome IS NULL;";

/// The one definition of a pass that counts toward the cap and the gap.
const COUNTS: &str = "outcome = 'completed'";

/// How long a settled pass is kept: past the gap and the pass day it counts
/// toward.
const KEEP_MS: i64 = 2 * 24 * 3_600_000;

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(TABLES)
}

/// One conversation as the pass decides on it.
#[derive(Clone, Debug)]
pub(super) struct Conversation {
    pub(super) session: Uuid,
    pub(super) principal: Uuid,
    pub(super) workspace: PathBuf,
    pub(super) last_activity_ms: i64,
    pub(super) user_chars: usize,
    pub(super) range_since_ms: Option<i64>,
    pub(super) range_since_note: u64,
}

/// A frozen pass whose outcome is not recorded.
pub(super) struct Frozen {
    pub(super) command: Uuid,
    pub(super) session: Uuid,
    pub(super) principal: Uuid,
    pub(super) prompt: String,
}

/// A person's message in a main-agent conversation, as the pass counts it.
pub(super) struct Said<'a> {
    pub(super) agent: AgentId,
    pub(super) session: Uuid,
    pub(super) workspace: &'a Path,
    pub(super) principal: Uuid,
    pub(super) command: Uuid,
    pub(super) chars: usize,
    pub(super) at_ms: i64,
    /// The notebook's length now, where a new conversation's range begins.
    pub(super) notes: u64,
    /// Whether the conversation had turns before the pass followed it. Its
    /// range then begins at this message, since the pass cannot tell what
    /// of the earlier text the agent already noted.
    pub(super) earlier: bool,
}

/// Whether the pass follows `session` already.
pub(super) fn follows(db: &Connection, session: Uuid) -> Result<bool, HostCatalogError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_memory_conversations WHERE session_id = ?1)",
        [session.to_string()],
        |row| row.get(0),
    )?)
}

/// Records `said`: its characters, once per command, and activity at its
/// time.
pub(super) fn message(db: &Connection, said: &Said<'_>) -> Result<(), HostCatalogError> {
    let workspace = said
        .workspace
        .to_str()
        .ok_or_else(|| HostCatalogError::Invalid("a workspace path is not UTF-8".to_owned()))?;
    db.execute(
        "INSERT INTO host_memory_conversations(session_id, agent_id, principal_id, workspace,
            last_activity_ms, user_chars, counted_command, range_since_note, range_since_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(session_id) DO UPDATE SET
            agent_id = excluded.agent_id,
            principal_id = excluded.principal_id,
            workspace = excluded.workspace,
            last_activity_ms = max(last_activity_ms, excluded.last_activity_ms),
            user_chars = user_chars + CASE WHEN counted_command IS excluded.counted_command
                THEN 0 ELSE excluded.user_chars END,
            counted_command = excluded.counted_command",
        params![
            said.session.to_string(),
            said.agent.to_string(),
            said.principal.to_string(),
            workspace,
            said.at_ms,
            count(said.chars)?,
            said.command.to_string(),
            count(said.notes)?,
            said.earlier.then_some(said.at_ms),
        ],
    )?;
    Ok(())
}

/// Removes what the pass keeps of `session` at `now_ms`: its conversation
/// and every pass of it that does not count. Its completed passes stay toward
/// the person's cap and gap until they are past [`KEEP_MS`], and go then
/// with everyone's.
pub(crate) fn forget(
    transaction: &rusqlite::Transaction<'_>,
    session: Uuid,
    now_ms: i64,
) -> Result<(), HostCatalogError> {
    let session = session.to_string();
    transaction.execute(
        &format!("DELETE FROM host_memory_passes WHERE session_id = ?1 AND NOT ({COUNTS})"),
        [&session],
    )?;
    transaction.execute(
        "DELETE FROM host_memory_conversations WHERE session_id = ?1",
        [&session],
    )?;
    prune(transaction, now_ms)
}

/// Deletes the settled passes past [`KEEP_MS`] at `now_ms`.
fn prune(db: &Connection, now_ms: i64) -> Result<(), HostCatalogError> {
    db.execute(
        "DELETE FROM host_memory_passes WHERE outcome IS NOT NULL AND finished_at_ms < ?1",
        [now_ms.saturating_sub(KEEP_MS)],
    )?;
    Ok(())
}

/// Records the agent's output in `session` at `at_ms`; nothing for a
/// conversation the pass does not follow.
pub(super) fn ended(db: &Connection, session: Uuid, at_ms: i64) -> Result<bool, HostCatalogError> {
    Ok(db.execute(
        "UPDATE host_memory_conversations SET last_activity_ms = max(last_activity_ms, ?2)
         WHERE session_id = ?1",
        params![session.to_string(), at_ms],
    )? == 1)
}

/// The conversation of `session`, when the pass follows it.
pub(super) fn conversation(
    db: &Connection,
    session: Uuid,
) -> Result<Option<Conversation>, HostCatalogError> {
    db.query_row(
        &format!("SELECT {CONVERSATION} FROM host_memory_conversations WHERE session_id = ?1"),
        [session.to_string()],
        conversation_row,
    )
    .optional()?
    .map(stored_conversation)
    .transpose()
}

/// `agent`'s conversations due to be asked again by `now_ms`, most
/// unreviewed text first.
pub(super) fn due(
    db: &Connection,
    agent: AgentId,
    now_ms: i64,
) -> Result<Vec<Conversation>, HostCatalogError> {
    select(db, "check_at_ms <= ?2", params![agent.to_string(), now_ms])
}

/// `agent`'s conversations with unreviewed text, most first.
pub(super) fn unreviewed(
    db: &Connection,
    agent: AgentId,
) -> Result<Vec<Conversation>, HostCatalogError> {
    select(db, "user_chars > 0", params![agent.to_string()])
}

fn select(
    db: &Connection,
    filter: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Conversation>, HostCatalogError> {
    db.prepare(&format!(
        "SELECT {CONVERSATION} FROM host_memory_conversations
         WHERE agent_id = ?1 AND {filter} ORDER BY user_chars DESC, session_id"
    ))?
    .query_map(params, conversation_row)?
    .map(|row| stored_conversation(row?))
    .collect()
}

/// Sets when `session` is asked again; `None` waits for new activity.
pub(super) fn check_at(
    db: &Connection,
    session: Uuid,
    at_ms: Option<i64>,
) -> Result<(), HostCatalogError> {
    db.execute(
        "UPDATE host_memory_conversations SET check_at_ms = ?2 WHERE session_id = ?1",
        params![session.to_string(), at_ms],
    )?;
    Ok(())
}

/// Stops asking again any of `agent`'s conversations due by `now_ms`, and
/// returns how many there were. After a run of the timer none may be, or the
/// timer would come due again at once.
pub(super) fn clear_due(
    db: &Connection,
    agent: AgentId,
    now_ms: i64,
) -> Result<usize, HostCatalogError> {
    Ok(db.execute(
        "UPDATE host_memory_conversations SET check_at_ms = NULL
         WHERE agent_id = ?1 AND check_at_ms <= ?2",
        params![agent.to_string(), now_ms],
    )?)
}

/// The earliest time any of `agent`'s conversations is asked again.
pub(super) fn next_check(db: &Connection, agent: AgentId) -> Result<Option<i64>, HostCatalogError> {
    Ok(db.query_row(
        "SELECT min(check_at_ms) FROM host_memory_conversations WHERE agent_id = ?1",
        [agent.to_string()],
        |row| row.get(0),
    )?)
}

/// For `principal`, when their last counted pass finished, in any
/// conversation, and how many counted passes `day` holds.
pub(super) fn person(
    db: &Connection,
    principal: Uuid,
    day: &str,
) -> Result<(Option<i64>, u32), HostCatalogError> {
    let (last, today): (Option<i64>, i64) = db.query_row(
        &format!(
            "SELECT max(finished_at_ms), coalesce(sum(pass_day = ?2), 0)
             FROM host_memory_passes WHERE principal_id = ?1 AND {COUNTS}"
        ),
        params![principal.to_string(), day],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let today = u32::try_from(today)
        .map_err(|error| HostCatalogError::Invalid(format!("pass count: {error}")))?;
    Ok((last, today))
}

/// The pass a stopped process left in `session`, if any.
pub(super) fn unsettled_in(
    db: &Connection,
    session: Uuid,
) -> Result<Option<Frozen>, HostCatalogError> {
    db.query_row(
        &format!(
            "SELECT {FROZEN} FROM host_memory_passes WHERE session_id = ?1 AND outcome IS NULL"
        ),
        [session.to_string()],
        frozen_row,
    )
    .optional()?
    .map(stored_frozen)
    .transpose()
}

/// Every pass a stopped process left in `agent`'s conversations.
pub(super) fn unsettled_of(
    db: &Connection,
    agent: AgentId,
) -> Result<Vec<Frozen>, HostCatalogError> {
    db.prepare(&format!(
        "SELECT {FROZEN} FROM host_memory_passes
         WHERE outcome IS NULL AND session_id IN
            (SELECT session_id FROM host_memory_conversations WHERE agent_id = ?1)
         ORDER BY frozen_at_ms, command_id"
    ))?
    .query_map([agent.to_string()], frozen_row)?
    .map(|row| stored_frozen(row?))
    .collect()
}

/// Freezes the next pass of `run` in `conversation`, with its message, and
/// returns its command. The person's unreviewed characters are kept with it,
/// so a pass that completes subtracts what it reviewed.
pub(super) fn freeze(
    db: &mut Connection,
    run: Uuid,
    conversation: &Conversation,
    kind: Kind,
    day: &str,
    prompt: &str,
    frozen_at_ms: i64,
) -> Result<Uuid, HostCatalogError> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let attempt: i64 = transaction.query_row(
        "SELECT count(*) FROM host_memory_passes WHERE run_id = ?1 AND session_id = ?2",
        params![run.to_string(), conversation.session.to_string()],
        |row| row.get(0),
    )?;
    let command = crate::stable_id::stable_id(&format!(
        "renoa.memory.note-pass.v1:{run}:{}:{attempt}",
        conversation.session
    ));
    transaction.execute(
        "INSERT INTO host_memory_passes(command_id, session_id, principal_id, run_id, kind,
            pass_day, frozen_at_ms, user_chars, prompt)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            command.to_string(),
            conversation.session.to_string(),
            conversation.principal.to_string(),
            run.to_string(),
            kind.as_str(),
            day,
            frozen_at_ms,
            count(conversation.user_chars)?,
            prompt,
        ],
    )?;
    transaction.commit()?;
    Ok(command)
}

/// Records how the pass of `command` ended, once: `false` when it was
/// recorded already. A completed pass ends its conversation's range: the next
/// begins at its frozen time and at `length`, the notebook's length after it,
/// and the characters it reviewed leave the count. Settled passes past
/// [`KEEP_MS`] go in the same transaction.
pub(super) fn settle(
    db: &mut Connection,
    command: Uuid,
    outcome: Outcome,
    noted: usize,
    length: u64,
    now_ms: i64,
) -> Result<bool, HostCatalogError> {
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let settled: Option<(String, i64, i64)> = transaction
        .query_row(
            "UPDATE host_memory_passes SET outcome = ?2, noted = ?3, finished_at_ms = ?4, prompt = NULL
             WHERE command_id = ?1 AND outcome IS NULL
             RETURNING session_id, frozen_at_ms, user_chars",
            params![command.to_string(), outcome.as_str(), count(noted)?, now_ms],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((session, frozen_at_ms, reviewed)) = settled else {
        return Ok(false);
    };
    if outcome == Outcome::Completed {
        transaction.execute(
            "UPDATE host_memory_conversations SET range_since_ms = ?2, range_since_note = ?3,
                user_chars = max(user_chars - ?4, 0)
             WHERE session_id = ?1",
            params![session, frozen_at_ms, count(length)?, reviewed],
        )?;
    }
    prune(&transaction, now_ms)?;
    transaction.commit()?;
    Ok(true)
}

/// For inspection: each person `agent` talks to, their counted passes in
/// `day`, and when any of their conversations is asked again.
pub(crate) fn people(
    db: &Connection,
    agent: AgentId,
    day: &str,
) -> Result<Vec<(Uuid, u32, Option<i64>)>, HostCatalogError> {
    db.prepare(&format!(
        "SELECT c.principal_id,
            (SELECT count(*) FROM host_memory_passes p
             WHERE p.principal_id = c.principal_id AND p.pass_day = ?2 AND p.{COUNTS}),
            min(c.check_at_ms)
         FROM host_memory_conversations c WHERE c.agent_id = ?1
         GROUP BY c.principal_id ORDER BY c.principal_id"
    ))?
    .query_map(params![agent.to_string(), day], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get(2)?))
    })?
    .map(|row| {
        let (principal, passes, next) = row?;
        Ok((
            parse(&principal)?,
            u32::try_from(passes)
                .map_err(|error| HostCatalogError::Invalid(format!("pass count: {error}")))?,
            next,
        ))
    })
    .collect()
}

const CONVERSATION: &str = "session_id, principal_id, workspace, last_activity_ms, user_chars, range_since_ms, range_since_note";
const FROZEN: &str = "command_id, session_id, principal_id, prompt";

type ConversationRow = (String, String, String, i64, i64, Option<i64>, i64);
type FrozenRow = (String, String, String, Option<String>);

fn conversation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn stored_conversation(row: ConversationRow) -> Result<Conversation, HostCatalogError> {
    let (session, principal, workspace, last_activity_ms, user_chars, range_since_ms, since_note) =
        row;
    Ok(Conversation {
        session: parse(&session)?,
        principal: parse(&principal)?,
        workspace: PathBuf::from(workspace),
        last_activity_ms,
        user_chars: usize::try_from(user_chars).map_err(invalid)?,
        range_since_ms,
        range_since_note: u64::try_from(since_note).map_err(invalid)?,
    })
}

fn frozen_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FrozenRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn stored_frozen(row: FrozenRow) -> Result<Frozen, HostCatalogError> {
    let (command, session, principal, prompt) = row;
    Ok(Frozen {
        command: parse(&command)?,
        session: parse(&session)?,
        principal: parse(&principal)?,
        prompt: prompt.ok_or_else(|| {
            HostCatalogError::Invalid("an unsettled note pass has no message".to_owned())
        })?,
    })
}

fn parse(value: &str) -> Result<Uuid, HostCatalogError> {
    Uuid::parse_str(value)
        .map_err(|error| HostCatalogError::Invalid(format!("stored note pass id: {error}")))
}

fn count<T: TryInto<i64>>(value: T) -> Result<i64, HostCatalogError>
where
    T::Error: std::fmt::Display,
{
    value.try_into().map_err(invalid)
}

fn invalid(error: impl std::fmt::Display) -> HostCatalogError {
    HostCatalogError::Invalid(format!("note pass count: {error}"))
}
