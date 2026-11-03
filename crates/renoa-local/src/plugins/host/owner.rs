//! `renoa.owner`: an agent reaches the owner on their phone. `ask_owner` asks
//! one question and ends the turn, and the owner's next message answers it;
//! `notify_owner` tells them one thing and the turn goes on. Its heartbeat
//! (see `heartbeat`) checks in every few hours, and only there may the agent
//! `set_today`, the line above the owner's feed.
//!
//! Each call writes one feed item and nothing else, so the item is the call's
//! whole effect: it is durable before the call succeeds, and a replay of the
//! same call converges on the same item. How loudly the owner's phone is told
//! is the item's urgency, which the agent must choose.

use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, Closing, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::host::{
    catalog,
    feed::{self, BODY_CHARS, CHOICE_CHARS, CHOICES, ForOwner},
};

pub(crate) mod heartbeat;
mod input;
#[cfg(test)]
mod tests;
mod today;

use input::{Input, Kind};

pub(crate) const PLUGIN_ID: &str = "renoa.owner";
pub(crate) const DESCRIPTION: &str = "Ask the owner a question or tell them something on their phone. When the owner asks you to check in on them, turn on your heartbeat, a check-in of your own every few hours, with configure_plugin settings {\"heartbeat_hours\": n}, 1 to 24 hours apart; 0 or settings {} turns it off.";
pub(crate) const ASK: &str = "ask_owner";
pub(crate) const NOTIFY: &str = "notify_owner";
pub(crate) const TODAY: &str = "set_today";
const BINDING_REVISION: &str = "renoa-owner-v1";
const NOT_RUN: &str =
    "Not run: your turn ended when you asked the owner. Put everything you need into one question.";
const URGENCY: &str = r#""urgency": "whenever" if nothing is lost when the owner sees it tomorrow, "soon" if they should see it within the hour, "now" only if waiting costs them something within minutes."#;

/// What one turn's owner tools act on.
#[derive(Clone, Debug)]
pub(crate) struct OwnerTurn {
    pub(crate) database: PathBuf,
    pub(crate) agent: AgentId,
    pub(crate) session: SessionId,
    pub(crate) command: Option<CommandId>,
}

/// What a resident owner tool says when the owner turned `renoa.owner` off
/// for this agent.
pub(crate) fn turned_off(tool: &str) -> String {
    format!(
        "{tool} is turned off for this agent, so nothing was sent. Say it in your reply instead."
    )
}

/// `ask_owner`, `notify_owner` and `set_today` for one turn. A replay
/// rewrites the same item or line, so each is safe to replay.
pub(crate) fn bindings(turn: OwnerTurn) -> [AgentToolBinding; 3] {
    let turn = Arc::new(turn);
    let [ask, notify] = [Kind::Ask, Kind::Notify].map(|kind| {
        AgentToolBinding::new(
            BINDING_REVISION,
            Arc::new(OwnerTool {
                kind,
                turn: Arc::clone(&turn),
                spec: spec(kind),
            }),
            EffectRecovery::SafeToReplay,
        )
    });
    let today = AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(today::TodayTool {
            turn,
            spec: today::spec(),
        }),
        EffectRecovery::SafeToReplay,
    );
    [ask, notify, today]
}

fn spec(kind: Kind) -> ToolSpec {
    let (least, most) = (CHOICES.start(), CHOICES.end());
    let urgency = json!({"type": "string", "enum": ["whenever", "soon", "now"]});
    match kind {
        Kind::Ask => ToolSpec {
            name: ASK.to_owned(),
            description: format!(
                "Ask the owner one question you cannot decide yourself; they see it on their phone. Your turn ends here, and their answer arrives later as a new message, so do not wait for it or ask again. Write any background in your message before this call. For a small question in an ordinary chat, just write it in your reply. Never ask for a password, token or key: plugin_manage sends a private setup link for that. Offer choices when the answer is one of a few. {URGENCY}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "minLength": 1, "maxLength": BODY_CHARS},
                    "choices": {
                        "type": "array",
                        "minItems": least,
                        "maxItems": most,
                        "items": {"type": "string", "minLength": 1, "maxLength": CHOICE_CHARS}
                    },
                    "urgency": urgency
                },
                "required": ["question", "urgency"],
                "additionalProperties": false
            }),
        },
        Kind::Notify => ToolSpec {
            name: NOTIFY.to_owned(),
            description: format!(
                "Tell the owner one short thing they would want to know even though they did not ask; they see it on their phone. Send nothing when there is nothing worth their attention. It does not end your turn. {URGENCY}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string", "minLength": 1, "maxLength": BODY_CHARS},
                    "urgency": urgency
                },
                "required": ["message", "urgency"],
                "additionalProperties": false
            }),
        },
    }
}

struct OwnerTool {
    kind: Kind,
    turn: Arc<OwnerTurn>,
    spec: ToolSpec,
}

impl Tool for OwnerTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let name = self.kind.name();
            if call.name != name {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{name}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input = input::parse(self.kind, &call.arguments)?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    format!("{name} was cancelled before anything was sent"),
                    false,
                ));
            }
            let turn = Arc::clone(&self.turn);
            let call_id = call.id;
            let saved = tokio::task::spawn_blocking(move || save(&turn, &call_id, &input))
                .await
                .map_err(|error| error.to_string())
                .and_then(|saved| saved.map_err(|error| error.to_string()));
            match saved {
                Ok(output) => Ok(output),
                Err(error) => {
                    renoa_telemetry::event(
                        "renoa.host",
                        "warn",
                        "owner_item_unsaved",
                        &json!({ "tool": name, "error": error }),
                    );
                    Err(ToolError::unavailable(not_saved(self.kind)))
                }
            }
        })
    }
}

fn not_saved(kind: Kind) -> String {
    let what = match kind {
        Kind::Ask => "question",
        Kind::Notify => "message",
    };
    format!(
        "The {what} could not be saved, so nothing was sent and your turn continues. Try once more; if it fails again, tell the owner in your reply."
    )
}

/// Writes the call's item in one transaction and returns what the model sees.
fn save(turn: &OwnerTurn, call: &str, input: &Input) -> Result<ToolOutput, feed::FeedError> {
    let (text, urgency) = match input {
        Input::Ask {
            question, urgency, ..
        } => (question, *urgency),
        Input::Notify { message, urgency } => (message, *urgency),
    };
    let item = ForOwner {
        agent: turn.agent,
        session: turn.session,
        command: turn.command,
        call,
        text,
        urgency,
    };
    let mut db = catalog::open_verified(&turn.database)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let output = match input {
        Input::Ask { choices, .. } => {
            feed::owner_asked(&tx, &item, choices.as_deref())?;
            let mut message = text.clone();
            for choice in choices.iter().flatten() {
                message.push_str("\n- ");
                message.push_str(choice);
            }
            ToolOutput {
                content: vec![ContentBlock::text(
                    "The owner has your question. Their answer comes as a new message.",
                )],
                details: None,
                is_error: false,
                closing: Some(Closing {
                    message,
                    not_run: NOT_RUN.to_owned(),
                }),
            }
        }
        Input::Notify { .. } => {
            feed::owner_told(&tx, &item)?;
            ToolOutput {
                content: vec![ContentBlock::text("The owner will see this in their feed.")],
                details: None,
                is_error: false,
                closing: None,
            }
        }
    };
    tx.commit()?;
    Ok(output)
}
