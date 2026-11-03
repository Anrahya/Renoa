//! `set_today`: the one sentence above the owner's feed on their home screen,
//! written by an agent's heartbeat and by nothing else.

use std::sync::Arc;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{NOTIFY, OwnerTurn, TODAY, heartbeat};
use crate::host::{
    catalog,
    feed::{self, TODAY_CHARS},
};

const EXAMPLE: &str = r#"{"sentence": "Your 3pm call moved to 4; nothing else needs you."}"#;

pub(super) struct TodayTool {
    pub(super) turn: Arc<OwnerTurn>,
    pub(super) spec: ToolSpec,
}

pub(super) fn spec() -> ToolSpec {
    ToolSpec {
        name: TODAY.to_owned(),
        description: format!(
            "Only during your heartbeat: set the one line the owner reads first on their home screen, about their day. It replaces the line before it and stays until something new needs them. It tells no one: use {NOTIFY} for what the owner should hear about."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "sentence": {"type": "string", "minLength": 1, "maxLength": TODAY_CHARS}
            },
            "required": ["sentence"],
            "additionalProperties": false
        }),
    }
}

impl Tool for TodayTool {
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
            if call.name != TODAY {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TODAY}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let sentence = parse(&call.arguments)?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    format!("{TODAY} was cancelled before anything was set"),
                    false,
                ));
            }
            let turn = Arc::clone(&self.turn);
            tokio::task::spawn_blocking(move || save(&turn, &sentence))
                .await
                .map_err(|error| ToolError::internal(error.to_string()))?
        })
    }
}

/// Sets the line in one transaction, if this turn is a heartbeat run.
fn save(turn: &OwnerTurn, sentence: &str) -> Result<ToolOutput, ToolError> {
    let unsaved = |error: &dyn std::fmt::Display| {
        renoa_telemetry::event(
            "renoa.host",
            "warn",
            "owner_item_unsaved",
            &json!({ "tool": TODAY, "error": error.to_string() }),
        );
        ToolError::unavailable(
            "The line could not be saved, so the owner's home is unchanged. Try once more; if it fails again, go on without it.",
        )
    };
    let mut db = catalog::open_verified(&turn.database).map_err(|error| unsaved(&error))?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| unsaved(&error))?;
    let heartbeat = match turn.command {
        Some(command) => {
            heartbeat::is_run(&tx, turn.agent, command).map_err(|error| unsaved(&error))?
        }
        None => false,
    };
    if !heartbeat {
        return Err(ToolError::invalid_input(format!(
            "{TODAY} only works during your heartbeat, and this turn is not one. To tell the owner something now, use {NOTIFY}."
        )));
    }
    feed::today_set(&tx, sentence).map_err(|error| unsaved(&error))?;
    tx.commit().map_err(|error| unsaved(&error))?;
    Ok(ToolOutput {
        content: vec![ContentBlock::text("The owner's home shows this line now.")],
        details: None,
        is_error: false,
        closing: None,
    })
}

/// The sentence a call sets. Every refusal names what was sent and shows a
/// correct call.
pub(super) fn parse(arguments: &Value) -> Result<String, ToolError> {
    let invalid = |message: String| Err(ToolError::invalid_input(message));
    let Some(fields) = arguments.as_object() else {
        return invalid(format!("{TODAY} takes one object. Example: {EXAMPLE}"));
    };
    if let Some(unknown) = fields.keys().find(|field| *field != "sentence") {
        return invalid(format!(
            "{TODAY} has no field \"{unknown}\". Its one field is sentence. Example: {EXAMPLE}"
        ));
    }
    let sentence = match fields.get("sentence") {
        None | Some(Value::Null) => {
            return invalid(format!(
                "{TODAY} needs \"sentence\": one line about the owner's day, at most {TODAY_CHARS} characters. Example: {EXAMPLE}"
            ));
        }
        Some(Value::String(sentence)) => sentence.trim(),
        Some(other) => {
            return invalid(format!(
                "\"sentence\" must be text; you sent {other}. Example: {EXAMPLE}"
            ));
        }
    };
    if sentence.is_empty() {
        return invalid(format!(
            "\"sentence\" is empty. Write one line about the owner's day. Example: {EXAMPLE}"
        ));
    }
    if sentence.contains(['\n', '\r']) {
        return invalid(format!(
            "\"sentence\" must be one line; yours has a line break. Example: {EXAMPLE}"
        ));
    }
    let length = sentence.chars().count();
    if length > TODAY_CHARS {
        return invalid(format!(
            "\"sentence\" is {length} characters; the limit is {TODAY_CHARS}. Keep it to the one line the owner should read first."
        ));
    }
    Ok(sentence.to_owned())
}
