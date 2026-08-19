use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use super::{CancellationEffect, UnsettledEffect};
use crate::{
    Command, EffectOutcome, KernelError, OperationId, RuntimeManifest, SessionId, SettledEffect,
    admission::parse_command_id,
    effect_store::parse_effect_id,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

pub(super) struct StoredOperation {
    pub(super) command_id: String,
    pub(super) command_json: String,
    pub(super) phase: String,
    pub(super) state_version: i64,
    pub(super) transition_version: i64,
    pub(super) manifest_json: Option<String>,
    pub(super) checkpoint_json: Option<String>,
    pub(super) current_effect_id: Option<String>,
    pub(super) input_effect_id: Option<String>,
    pub(super) outcome_json: Option<String>,
}

pub(super) fn load_operation(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<StoredOperation, KernelError> {
    connection
        .query_row(
            "SELECT o.command_id, c.content_json, o.phase, o.state_version,
                    o.transition_version, o.manifest_json, o.checkpoint_json,
                    o.current_effect_id, o.input_effect_id, o.outcome_json
             FROM operations AS o
             JOIN commands AS c
               ON c.session_id = o.session_id AND c.command_id = o.command_id
             WHERE o.session_id = ?1 AND o.operation_id = ?2",
            params![session_id.to_string(), operation_id.to_string()],
            |row| {
                Ok(StoredOperation {
                    command_id: row.get(0)?,
                    command_json: row.get(1)?,
                    phase: row.get(2)?,
                    state_version: row.get(3)?,
                    transition_version: row.get(4)?,
                    manifest_json: row.get(5)?,
                    checkpoint_json: row.get(6)?,
                    current_effect_id: row.get(7)?,
                    input_effect_id: row.get(8)?,
                    outcome_json: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt("cancelled operation is missing".to_owned()))
}

pub(super) fn load_cancellation_effect(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    phase: OperationPhase,
    current_effect_id: Option<&str>,
    input_effect_id: Option<&str>,
    manifest: &RuntimeManifest,
) -> Result<Option<CancellationEffect>, KernelError> {
    let (effect_id, expected_status, kind) = match phase {
        OperationPhase::NeedDecision => {
            let Some(effect_id) = input_effect_id else {
                if current_effect_id.is_some() {
                    return Err(KernelError::Corrupt(
                        "decision phase contains a current effect".to_owned(),
                    ));
                }
                return Ok(None);
            };
            (effect_id, "settled", EffectKind::Settled)
        }
        OperationPhase::EffectIntent => (
            require_effect_id(current_effect_id, input_effect_id)?,
            "intent_committed",
            EffectKind::NotDispatched,
        ),
        OperationPhase::OutcomeUnknown => (
            require_effect_id(current_effect_id, input_effect_id)?,
            "outcome_unknown",
            EffectKind::OutcomeUnknown,
        ),
        _ => {
            return Err(KernelError::Corrupt(format!(
                "phase `{}` cannot be closed by cancellation",
                phase.as_str()
            )));
        }
    };
    let effect_id = parse_effect_id(effect_id)?;
    let (binding, revision, request, status, outcome) = connection
        .query_row(
            "SELECT binding, binding_revision, request_json, status, outcome_json
             FROM effects WHERE operation_id = ?1 AND effect_id = ?2",
            params![operation_id.to_string(), effect_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt("cancellation effect is missing".to_owned()))?;
    if status != expected_status || manifest.effect_bindings.get(&binding) != Some(&revision) {
        return Err(KernelError::Corrupt(
            "cancellation effect differs from durable operation state".to_owned(),
        ));
    }
    let request: Value = serde_json::from_str(&request).map_err(json_error)?;
    let unsettled = || UnsettledEffect {
        effect_id,
        binding: binding.clone(),
        binding_revision: revision.clone(),
        request: request.clone(),
    };
    match kind {
        EffectKind::NotDispatched if outcome.is_none() => {
            Ok(Some(CancellationEffect::NotDispatched(unsettled())))
        }
        EffectKind::OutcomeUnknown if outcome.is_none() => {
            Ok(Some(CancellationEffect::OutcomeUnknown(unsettled())))
        }
        EffectKind::Settled => {
            let outcome = outcome
                .map(|value| serde_json::from_str::<EffectOutcome>(&value).map_err(json_error))
                .transpose()?
                .ok_or_else(|| KernelError::Corrupt("settled effect has no outcome".to_owned()))?;
            Ok(Some(CancellationEffect::Settled(SettledEffect {
                effect_id,
                binding,
                binding_revision: revision,
                request,
                outcome,
            })))
        }
        EffectKind::NotDispatched | EffectKind::OutcomeUnknown => Err(KernelError::Corrupt(
            "unsettled cancellation effect contains an outcome".to_owned(),
        )),
    }
}

#[derive(Clone, Copy)]
enum EffectKind {
    NotDispatched,
    Settled,
    OutcomeUnknown,
}

fn require_effect_id<'a>(
    current_effect_id: Option<&'a str>,
    input_effect_id: Option<&str>,
) -> Result<&'a str, KernelError> {
    if input_effect_id.is_some() {
        return Err(KernelError::Corrupt(
            "active effect phase contains settled input".to_owned(),
        ));
    }
    current_effect_id
        .ok_or_else(|| KernelError::Corrupt("active effect identity is missing".to_owned()))
}

pub(super) fn decode_manifest(value: Option<String>) -> Result<RuntimeManifest, KernelError> {
    value
        .map(|value| serde_json::from_str(&value).map_err(json_error))
        .transpose()?
        .ok_or_else(|| KernelError::Corrupt("cancelled operation has no manifest".to_owned()))
}

pub(super) fn decode_command(command_id: &str, command_json: &str) -> Result<Command, KernelError> {
    let command: Command = serde_json::from_str(command_json).map_err(json_error)?;
    if command.command_id() != parse_command_id(command_id)? {
        return Err(KernelError::Corrupt(
            "cancelled operation command identity differs from stored content".to_owned(),
        ));
    }
    Ok(command)
}
