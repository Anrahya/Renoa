//! `follow_up`: an agent's note to itself, sent back into the conversation it
//! was set in after some minutes, once.
//!
//! It is a one-time automation of the calling agent with that conversation as
//! its origin, created like `automation_manage` creates one, and marked in
//! `host_automation_follow_ups`. A run that succeeded in its conversation
//! deletes it through the deletion and purge path; one that failed, or that
//! had no conversation left to return to, stays, disarmed, with its result,
//! as any one-time automation does. One the agent re-armed stays too.

use std::{path::Path, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use rusqlite::{Transaction, TransactionBehavior};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AutomationError, AutomationRecord, AutomationSchedule, AutomationSpec,
    receipts::AutomationActor, store,
};
use crate::{
    capabilities::{AUTOMATION_MANAGE, FOLLOW_UP},
    host::{HostConfig, catalog, feed::RunEnded},
};

const NOTE_CHARS: usize = 500;
const LEAST_MINUTES: u64 = 5;
const MOST_MINUTES: u64 = 30 * 24 * 60;
/// Follow-ups one agent may have waiting to run.
pub(super) const WAITING_MAX: i64 = 20;
const NAME_CHARS: usize = 60;
const EXAMPLE: &str = r#"{"note": "Ask whether the table was booked", "after_minutes": 1440}"#;

pub(in crate::host) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_automation_follow_ups (
            automation_id TEXT PRIMARY KEY REFERENCES host_automations(id)
        ) STRICT;",
    )?;
    Ok(())
}

/// Turns a schema 51 catalog back into schema 50's tables.
#[cfg(test)]
pub(crate) fn restore_schema_50(connection: &rusqlite::Connection) {
    connection
        .execute_batch("DROP TABLE host_automation_follow_ups;")
        .expect("restore schema 50");
}

pub(crate) fn binding(
    host: Arc<HostConfig>,
    actor: AgentId,
    session: SessionId,
    command: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new(
        "renoa-follow-up-v1",
        Arc::new(FollowUp {
            host,
            actor,
            session,
            command,
            spec: spec(),
        }),
        EffectRecovery::SafeToReplay,
    )
}

fn spec() -> ToolSpec {
    ToolSpec {
        name: FOLLOW_UP.to_owned(),
        description: format!(
            "Check back on something later in this conversation. After after_minutes, your note comes back to you here as a new message, and you act on it then. Whenever you tell the person you will check, remind them of, or do something later, call this in the same turn, before you reply. Write the note as an instruction to your future self: what to do, why, and the facts you will need. It runs once, and once it has run here it is removed. For a repeating schedule or an exact date, use {AUTOMATION_MANAGE}."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "note": {"type": "string", "minLength": 1, "maxLength": NOTE_CHARS},
                "after_minutes": {"type": "integer", "minimum": LEAST_MINUTES, "maximum": MOST_MINUTES}
            },
            "required": ["note", "after_minutes"],
            "additionalProperties": false
        }),
    }
}

struct FollowUp {
    host: Arc<HostConfig>,
    actor: AgentId,
    session: SessionId,
    command: Option<CommandId>,
    spec: ToolSpec,
}

impl Tool for FollowUp {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != FOLLOW_UP {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{FOLLOW_UP}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let request = parse(&call.arguments)?;
            let operation = crate::stable_id::stable_id(&format!(
                "renoa.automation.follow_up.v1:{}",
                crate::mcp::oauth_operation_id(self.session, self.command, &call.id)
            ));
            let now_ms = crate::TurnObservation::now()
                .map_err(|error| ToolError::internal(error.to_string()))?
                .unix_milliseconds();
            let (database, actor, session) = (self.host.database.clone(), self.actor, self.session);
            let saved = tokio::task::spawn_blocking(move || {
                save(
                    &database,
                    actor,
                    session,
                    operation,
                    &request,
                    now_ms,
                    &cancellation,
                )
            })
            .await
            .map_err(|error| ToolError::internal(error.to_string()))?;
            match saved {
                Ok(Saved::Set(record)) => {
                    let due = jiff::Timestamp::from_millisecond(record.next_due_ms)
                        .map_err(|error| ToolError::internal(error.to_string()))?;
                    Ok(ToolOutput {
                        content: vec![ContentBlock::text(
                            json!({"id": record.id, "revision": record.revision, "due": due.to_string()})
                                .to_string(),
                        )],
                        details: None,
                        is_error: false,
                        closing: None,
                    })
                }
                Ok(Saved::Full) => Err(ToolError::invalid_input(format!(
                    "You already have {WAITING_MAX} follow-ups waiting; the limit is {WAITING_MAX}. Let one run, or remove one with {AUTOMATION_MANAGE} (action delete), then try again."
                ))),
                Err(AutomationError::Conflict) => Err(ToolError::invalid_input(format!(
                    "This call's id already set a different follow-up, so nothing new was set. Call {FOLLOW_UP} again as a new call."
                ))),
                Err(AutomationError::Cancelled) => Err(ToolError::cancelled(
                    format!("{FOLLOW_UP} was cancelled before anything was set"),
                    false,
                )),
                Err(error) => {
                    renoa_telemetry::event(
                        "renoa.host",
                        "warn",
                        "follow_up_unsaved",
                        &json!({ "error": error.to_string() }),
                    );
                    Err(ToolError::unavailable(
                        "The follow-up could not be saved, so nothing will come back to you. Try once more; if it fails again, tell the person you could not set it.",
                    ))
                }
            }
        })
    }
}

/// What one call asks for, and the request its receipt keeps: the call's own
/// fields, so a replay after a crash, at a later time, still matches it.
#[derive(serde::Serialize)]
struct Request {
    note: String,
    after_minutes: u64,
}

enum Saved {
    Set(AutomationRecord),
    Full,
}

/// Creates the follow-up and its mark in one transaction, after the limit
/// check; a replayed operation returns what it created.
fn save(
    path: &Path,
    agent: AgentId,
    session: SessionId,
    operation: Uuid,
    request: &Request,
    now_ms: i64,
    cancellation: &CancellationToken,
) -> Result<Saved, AutomationError> {
    let actor = AutomationActor::Agent {
        id: agent,
        session: Some(session),
    };
    store::active(cancellation)?;
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let receipt = serde_json::to_string(request)?;
    if let Some(record) = actor.replay(&tx, operation, &receipt)? {
        return Ok(Saved::Set(record));
    }
    let waiting: i64 = tx.query_row(
        "SELECT count(*) FROM host_automation_follow_ups f
         JOIN host_automations a ON a.id = f.automation_id
         WHERE a.agent_id = ?1 AND a.enabled = 1",
        [agent.to_string()],
        |row| row.get(0),
    )?;
    if waiting >= WAITING_MAX {
        return Ok(Saved::Full);
    }
    let minutes = i64::try_from(request.after_minutes)
        .map_err(|_| AutomationError::Invalid("after_minutes is out of range".to_owned()))?;
    let at = jiff::Timestamp::from_millisecond(now_ms.saturating_add(minutes * 60_000))
        .map_err(|error| AutomationError::Invalid(error.to_string()))?;
    let spec = AutomationSpec {
        agent_id: agent,
        name: name(&request.note),
        prompt: request.note.clone(),
        schedule: AutomationSchedule::Once { at: at.to_string() },
        enabled: true,
    };
    let record = store::create(&tx, actor, operation, spec, now_ms)?;
    tx.execute(
        "INSERT INTO host_automation_follow_ups(automation_id) VALUES(?1)",
        [record.id.to_string()],
    )?;
    store::active(cancellation)?;
    actor.save(&tx, operation, &receipt, &record)?;
    tx.commit()?;
    Ok(Saved::Set(record))
}

/// Deletes follow-up `automation` once its run ended as `ended`, if that run
/// succeeded in its conversation and the follow-up was not armed again; the
/// purge that follows in the same transaction removes its data.
pub(super) fn retire(
    tx: &Transaction<'_>,
    automation: Uuid,
    ended: Option<RunEnded>,
) -> Result<(), AutomationError> {
    if !matches!(
        ended,
        Some(RunEnded::Succeeded(super::RunLanding::Conversation))
    ) {
        return Ok(());
    }
    let id = automation.to_string();
    let marked = tx.execute(
        "INSERT OR IGNORE INTO host_automation_deletions(automation_id)
         SELECT f.automation_id FROM host_automation_follow_ups f
         JOIN host_automations a ON a.id = f.automation_id
         WHERE f.automation_id = ?1 AND a.enabled = 0",
        [&id],
    )?;
    if marked > 0 {
        tx.execute(
            "UPDATE host_automations SET revision = revision + 1 WHERE id = ?1",
            [&id],
        )?;
    }
    Ok(())
}

/// The automation's name: "Follow-up: " and the note's first line, cut to
/// [`NAME_CHARS`].
fn name(note: &str) -> String {
    let line = note.lines().next().unwrap_or_default().trim();
    let mut name: String = line.chars().take(NAME_CHARS).collect();
    if line.chars().count() > NAME_CHARS {
        name.push('…');
    }
    format!("Follow-up: {name}")
}

/// The note and minutes a call asks for. Every refusal says what was wrong
/// and how to fix the call.
fn parse(arguments: &Value) -> Result<Request, ToolError> {
    let invalid = |message: String| Err(ToolError::invalid_input(message));
    let Some(fields) = arguments.as_object() else {
        return invalid(format!("{FOLLOW_UP} takes one object. Example: {EXAMPLE}"));
    };
    if let Some(unknown) = fields
        .keys()
        .find(|field| !matches!(field.as_str(), "note" | "after_minutes"))
    {
        return invalid(format!(
            "{FOLLOW_UP} has no field \"{unknown}\". Its fields are note, after_minutes. Example: {EXAMPLE}"
        ));
    }
    let note = match fields.get("note") {
        None | Some(Value::Null) => {
            return invalid(format!(
                "{FOLLOW_UP} needs \"note\": what to do when it comes back to you. Example: {EXAMPLE}"
            ));
        }
        Some(Value::String(note)) => note.trim(),
        Some(other) => {
            return invalid(format!(
                "\"note\" must be text; you sent {other}. Example: {EXAMPLE}"
            ));
        }
    };
    if note.is_empty() {
        return invalid(format!(
            "\"note\" is empty. Write what to do when it comes back to you. Example: {EXAMPLE}"
        ));
    }
    let length = note.chars().count();
    if length > NOTE_CHARS {
        return invalid(format!(
            "\"note\" is {length} characters; the limit is {NOTE_CHARS}. Keep what to do and the facts you will need."
        ));
    }
    let after_minutes = match fields.get("after_minutes") {
        None | Some(Value::Null) => {
            return invalid(format!(
                "{FOLLOW_UP} needs \"after_minutes\": how many minutes from now, {LEAST_MINUTES} to {MOST_MINUTES}. Example: {EXAMPLE}"
            ));
        }
        Some(Value::Number(number)) => number,
        Some(other) => {
            return invalid(format!(
                "\"after_minutes\" must be a whole number of minutes; you sent {other}. Example: {EXAMPLE}"
            ));
        }
    };
    match after_minutes.as_u64() {
        Some(minutes) if minutes < LEAST_MINUTES => invalid(format!(
            "\"after_minutes\" is {minutes}; the least is {LEAST_MINUTES}. Example: {EXAMPLE}"
        )),
        Some(minutes) if minutes > MOST_MINUTES => invalid(format!(
            "\"after_minutes\" is {minutes}; the most is {MOST_MINUTES}, 30 days. For a later date, use {AUTOMATION_MANAGE} with kind=once."
        )),
        Some(minutes) => Ok(Request {
            note: note.to_owned(),
            after_minutes: minutes,
        }),
        None => invalid(format!(
            "\"after_minutes\" must be a whole number of minutes from {LEAST_MINUTES} to {MOST_MINUTES}; you sent {after_minutes}. Example: {EXAMPLE}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{name, parse};

    fn refusal(arguments: &serde_json::Value) -> String {
        match parse(arguments) {
            Ok(_) => panic!("{arguments} should be refused"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn every_refusal_says_what_was_wrong_and_how_to_fix_the_call() {
        const EXAMPLE: &str =
            r#"Example: {"note": "Ask whether the table was booked", "after_minutes": 1440}"#;
        assert_eq!(
            refusal(&json!({"note": "Ask", "after_minutes": 3})),
            format!("\"after_minutes\" is 3; the least is 5. {EXAMPLE}")
        );
        assert_eq!(
            refusal(&json!({"note": "Ask", "after_minutes": 43_201})),
            "\"after_minutes\" is 43201; the most is 43200, 30 days. For a later date, use automation_manage with kind=once."
        );
        assert_eq!(
            refusal(&json!({"note": "Ask", "after_minutes": 1.5})),
            format!(
                "\"after_minutes\" must be a whole number of minutes from 5 to 43200; you sent 1.5. {EXAMPLE}"
            )
        );
        assert_eq!(
            refusal(&json!({"note": "Ask", "after_minutes": "1440"})),
            format!(
                "\"after_minutes\" must be a whole number of minutes; you sent \"1440\". {EXAMPLE}"
            )
        );
        assert_eq!(
            refusal(&json!({"note": "Ask"})),
            format!(
                "follow_up needs \"after_minutes\": how many minutes from now, 5 to 43200. {EXAMPLE}"
            )
        );
        assert_eq!(
            refusal(&json!({"after_minutes": 60})),
            format!("follow_up needs \"note\": what to do when it comes back to you. {EXAMPLE}")
        );
        assert_eq!(
            refusal(&json!({"note": "  ", "after_minutes": 60})),
            format!("\"note\" is empty. Write what to do when it comes back to you. {EXAMPLE}")
        );
        assert_eq!(
            refusal(&json!({"note": "x".repeat(501), "after_minutes": 60})),
            "\"note\" is 501 characters; the limit is 500. Keep what to do and the facts you will need."
        );
        assert_eq!(
            refusal(&json!({"note": "Ask", "after_minutes": 60, "when": "tomorrow"})),
            format!(
                "follow_up has no field \"when\". Its fields are note, after_minutes. {EXAMPLE}"
            )
        );
        assert_eq!(
            refusal(&json!("Ask tomorrow")),
            format!("follow_up takes one object. {EXAMPLE}")
        );
    }

    #[test]
    fn a_follow_up_is_named_by_the_first_line_of_its_note() {
        assert_eq!(
            name("Ask whether the table was booked\nIt was for six."),
            "Follow-up: Ask whether the table was booked"
        );
        assert_eq!(
            name(&"a".repeat(61)),
            format!("Follow-up: {}…", "a".repeat(60))
        );
    }
}
