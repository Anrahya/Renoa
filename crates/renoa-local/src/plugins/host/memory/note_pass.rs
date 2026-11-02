//! `renoa.memory`'s note pass: Mnem's backup note-taker, run as a turn of the
//! main agent in its own conversation with a person once it has gone quiet.
//!
//! Mnem decides when (`mnem::pass::due`), writes the one message the pass
//! sends (`Memory::note_pass`), and owns its wording; the Host keeps the
//! counts it decides from ([`store`]), asks it again when it said to wait
//! ([`schedule`]'s timer, a trigger of this plugin), and runs the pass:
//!
//! 1. A pass claims its conversation, so it never runs while a turn does, and
//!    Mnem decides again under the claim. The pass is frozen, with its
//!    message, under a derived command id before that command is admitted,
//!    and settled before the claim is released ([`runner`], [`settle`]).
//! 2. The command is a receipted prompt in the conversation's own session,
//!    with the turn's system prompt and tools byte for byte, so the cached
//!    prefix holds. Only the four calls the pass needs run ([`guard`]).
//! 3. Its text is discarded: every later request shows one receipt in place
//!    of its exchange ([`receipt`]).
//! 4. The person's next message cancels it: their surface's claim cancels the
//!    pass's command, which aborts an in-flight model call, and opens the
//!    conversation once the pass has settled. Notes it wrote stay, and a
//!    cancelled pass does not count toward the person's passes.
//! 5. A pass a stopped process left is closed, never resumed: its command is
//!    cancelled, so a note in flight then is never written twice, and it
//!    counts only if its command had completed. The next pass lists what was
//!    noted.
//!
//! The nightly run sweeps every main-agent conversation with unreviewed text
//! before it compacts and dreams ([`sweep`]).

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use renoa_agent::ContentBlock;
use renoa_kernel::AgentId;
use serde_json::json;
use uuid::Uuid;

use super::nightly::driver;
use crate::{host::catalog, plugins::PluginError};

mod guard;
mod input;
mod receipt;
mod report;
mod runner;
mod schedule;
mod settle;
pub(crate) mod store;

pub(crate) use guard::guard;
pub(crate) use receipt::{REVISION as RECEIPT_REVISION, ReceiptProjector};
pub(crate) use runner::{run, sweep};
pub(crate) use schedule::{timer_id, wanted};
pub(crate) use settle::close_stale;

/// Which pass: one after a quiet spell, or the nightly sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Daytime,
    Sweep,
}

impl Kind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Daytime => "daytime",
            Self::Sweep => "sweep",
        }
    }
}

/// One Agent session, as the pass's bookkeeping needs it.
pub(crate) struct SessionRef {
    pub(crate) database: PathBuf,
    pub(crate) data_directory: PathBuf,
    pub(crate) agent: AgentId,
    pub(crate) session: Uuid,
    pub(crate) workspace: PathBuf,
}

/// Whether a message in `at` would be the first of the conversation the pass
/// follows: it is the main agent's, and the pass does not follow it yet.
///
/// # Errors
///
/// As [`message`].
pub(crate) fn first_followed(at: &SessionRef) -> Result<bool, PluginError> {
    let db = catalog::open_verified(&at.database)?;
    Ok(driver(&db)? == Some(at.agent) && !store::follows(&db, at.session)?)
}

/// Each person the main agent `agent` has a conversation with that the pass
/// follows, and the one of them last active, the earliest id on a tie.
///
/// # Errors
///
/// Returns catalog failures and stored ids that do not parse.
pub(crate) fn latest_conversations(
    db: &rusqlite::Connection,
    agent: AgentId,
) -> Result<Vec<(Uuid, Uuid)>, PluginError> {
    db.prepare(
        "SELECT c.principal_id, c.session_id FROM host_memory_conversations c
         WHERE c.agent_id = ?1 AND c.session_id = (
             SELECT x.session_id FROM host_memory_conversations x
             WHERE x.agent_id = ?1 AND x.principal_id = c.principal_id
             ORDER BY x.last_activity_ms DESC, x.session_id LIMIT 1)
         ORDER BY c.principal_id",
    )?
    .query_map([agent.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?
    .map(|row| {
        let (principal, session) = row?;
        let parse = |value: &str| {
            Uuid::parse_str(value)
                .map_err(|error| PluginError::Invalid(format!("a stored conversation id: {error}")))
        };
        Ok((parse(&principal)?, parse(&session)?))
    })
    .collect()
}

/// The characters of text in `content`, as the pass counts a message.
pub(crate) fn chars(content: &[ContentBlock]) -> usize {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.chars().count(),
            ContentBlock::Image { .. } => 0,
        })
        .sum()
}

/// The person's message `command`, of `chars` characters received at
/// `at_ms`, once the kernel admitted it in the main agent's conversation
/// `at`: counts it toward the conversation's unreviewed text, once per
/// command, and asks again when a pass is due. A conversation the pass begins
/// to follow with turns `earlier` is reviewed from this message on. Nothing
/// for any other agent.
///
/// # Errors
///
/// Returns catalog, notebook and time failures; the caller logs them, since a
/// message is never refused for the pass's bookkeeping.
pub(crate) fn message(
    at: &SessionRef,
    principal: Uuid,
    command: Uuid,
    (chars, at_ms): (usize, i64),
    earlier: bool,
) -> Result<(), PluginError> {
    let db = catalog::open_verified(&at.database)?;
    if driver(&db)? != Some(at.agent) {
        return Ok(());
    }
    store::message(
        &db,
        &store::Said {
            agent: at.agent,
            session: at.session,
            workspace: &at.workspace,
            principal,
            command,
            chars,
            at_ms,
            notes: input::notes(&at.data_directory, principal)?,
            earlier,
        },
    )?;
    schedule::reschedule(&db, at.session, at_ms)?;
    Ok(())
}

/// The main agent's output in the conversation `at` as of now: its quiet
/// spell starts again.
///
/// # Errors
///
/// As [`message`].
pub(crate) fn turn_ended(at: &SessionRef) -> Result<(), PluginError> {
    let db = catalog::open_verified(&at.database)?;
    if driver(&db)? != Some(at.agent) {
        return Ok(());
    }
    let now_ms = now_ms(&at.database);
    if store::ended(&db, at.session, now_ms)? {
        schedule::reschedule(&db, at.session, now_ms)?;
    }
    Ok(())
}

/// For inspection: each person the main agent's followed conversations are
/// with, their counted passes in today's pass day, and when one of their
/// conversations is asked again. Nothing while `renoa.memory` drives no
/// notebook. A catalog or notebook zone that cannot be read shows nothing
/// and is logged, so the pass never fails inspection.
pub(crate) fn observed(
    db: &rusqlite::Connection,
    database: &Path,
) -> Vec<crate::ObservedMemoryPasses> {
    people_today(db, database).unwrap_or_else(|error| {
        event(
            "warn",
            "memory_passes_unobserved",
            &json!({"error": error.to_string()}),
        );
        Vec::new()
    })
}

fn people_today(
    db: &rusqlite::Connection,
    database: &Path,
) -> Result<Vec<crate::ObservedMemoryPasses>, PluginError> {
    let Some(agent) = driver(db)? else {
        return Ok(Vec::new());
    };
    let (_, now) = schedule::zone_and_time(db, now_ms(database))?;
    let day = mnem::pass::day(now).to_string();
    Ok(store::people(db, agent, &day)?
        .into_iter()
        .map(
            |(principal_id, passes_today, next_check_ms)| crate::ObservedMemoryPasses {
                principal_id,
                pass_day: day.clone(),
                passes_today,
                next_check_ms,
            },
        )
        .collect())
}

/// Runs `work` on the catalog off the async executor.
async fn catalog_call<T: Send + 'static>(
    database: &Path,
    work: impl FnOnce(&mut rusqlite::Connection) -> Result<T, PluginError> + Send + 'static,
) -> Result<T, PluginError> {
    let database = database.to_path_buf();
    tokio::task::spawn_blocking(move || work(&mut catalog::open_verified(&database)?)).await?
}

fn unavailable(error: &crate::LocalHostError) -> PluginError {
    PluginError::Unavailable(error.to_string())
}

/// Logs one note pass event; tests read them back with the memory jobs'.
fn event(level: &'static str, name: &'static str, fields: &serde_json::Value) {
    super::pass::event(level, name, fields);
}

/// Logs that the command `request`, `why` it is not refused, waits again for
/// a pass in `session` that has not let go.
pub(crate) fn delays_command(session: Uuid, request: Uuid, why: &'static str) {
    event(
        "warn",
        "memory_pass_delays_command",
        &json!({"session_id": session, "command_id": request, "why": why}),
    );
}

/// Logs that whether the command `request` is an automation run being
/// delivered could not be read, so it waits for the pass in `session` again.
pub(crate) fn delivery_unread(session: Uuid, request: Uuid, error: &crate::LocalHostError) {
    event(
        "warn",
        "memory_pass_delivery_unread",
        &json!({"session_id": session, "command_id": request, "error": error.to_string()}),
    );
}

/// Logs that a message's or a turn's pass bookkeeping could not be recorded.
pub(crate) fn unrecorded(session: Uuid, error: &PluginError) {
    event(
        "warn",
        "memory_pass_activity_unrecorded",
        &json!({"session_id": session, "error": error.to_string()}),
    );
}

/// The Host's wall clock.
#[cfg(not(test))]
pub(crate) fn now_ms(_database: &Path) -> i64 {
    Timestamp::now().as_millisecond()
}

/// The Host's wall clock, unless the test set this Host's.
#[cfg(test)]
pub(crate) fn now_ms(database: &Path) -> i64 {
    clock::get(database).unwrap_or_else(|| Timestamp::now().as_millisecond())
}

#[cfg(test)]
pub(crate) mod clock {
    //! The time a test Host's turns end at, keyed by its catalog.

    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static CLOCKS: Mutex<Option<HashMap<PathBuf, i64>>> = Mutex::new(None);

    pub(crate) fn set(database: &Path, at_ms: i64) {
        CLOCKS
            .lock()
            .expect("clock lock")
            .get_or_insert_with(HashMap::new)
            .insert(database.to_path_buf(), at_ms);
    }

    pub(super) fn get(database: &Path) -> Option<i64> {
        CLOCKS
            .lock()
            .expect("clock lock")
            .as_ref()
            .and_then(|clocks| clocks.get(database).copied())
    }
}
