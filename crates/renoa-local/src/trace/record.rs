use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_agent::{
    AgentEvent, AssistantDelta, ModelFailureCode, ModelRequest, ModelResponse, ToolCall,
};
use serde_json::{Value, json};

use super::{TraceError, writer::TraceEntry};

#[derive(Default)]
pub(super) struct TraceState {
    sequence: i64,
    model_timings: HashMap<String, ModelTiming>,
    tool_starts: HashMap<String, i64>,
    pub(super) finished: bool,
}

struct ModelTiming {
    started_us: i64,
    first_output_us: Option<i64>,
}

impl TraceState {
    pub(super) fn next_sequence(&mut self) -> Result<i64, TraceError> {
        self.sequence = self.sequence.checked_add(1).ok_or_else(|| {
            TraceError::Incompatible("trace event sequence overflowed i64".to_owned())
        })?;
        Ok(self.sequence)
    }

    pub(super) fn agent_event(
        &mut self,
        event: AgentEvent,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        match event {
            AgentEvent::AgentStart => lifecycle("agent_start", occurred_at_ms, elapsed_us),
            AgentEvent::TurnStart => lifecycle("turn_start", occurred_at_ms, elapsed_us),
            AgentEvent::TurnEnd => lifecycle("turn_end", occurred_at_ms, elapsed_us),
            AgentEvent::AgentEnd => lifecycle("agent_end", occurred_at_ms, elapsed_us),
            AgentEvent::MessageStart { role } => {
                TraceEntry::new("message", "start", occurred_at_ms, elapsed_us)
                    .name(role_name(role))
            }
            AgentEvent::MessageUpdate {
                content_index,
                delta,
            } => TraceEntry::new("message", "chunk", occurred_at_ms, elapsed_us)
                .payload(&json!({ "content_index": content_index, "delta": delta })),
            AgentEvent::MessageAbort => {
                TraceEntry::new("message", "aborted", occurred_at_ms, elapsed_us)
            }
            AgentEvent::MessageEnd { message } => {
                TraceEntry::new("message", "end", occurred_at_ms, elapsed_us)
                    .payload(&to_value(message))
            }
            AgentEvent::ModelRequestStart { .. }
            | AgentEvent::ModelProviderRequest { .. }
            | AgentEvent::ModelProviderResponse { .. }
            | AgentEvent::ModelRequestChunk { .. }
            | AgentEvent::ModelRequestEnd { .. }
            | AgentEvent::ModelRequestFailed { .. }
            | AgentEvent::ModelRetryAttempt { .. } => {
                self.model_agent_event(event, occurred_at_ms, elapsed_us)
            }
            AgentEvent::ToolExecutionStart { call } => {
                self.tool_started(call, occurred_at_ms, elapsed_us)
            }
            AgentEvent::ToolExecutionUpdate { call, update } => {
                TraceEntry::new("tool", "execution_update", occurred_at_ms, elapsed_us)
                    .correlation(call.id)
                    .name(call.name)
                    .payload(&to_value(update))
            }
            AgentEvent::ToolExecutionEnd { call, result } => {
                self.tool_finished(call, occurred_at_ms, elapsed_us, result)
            }
            AgentEvent::ToolExecutionOutcomeUnknown { call, error } => {
                self.tool_unknown(call, occurred_at_ms, elapsed_us, error)
            }
        }
    }

    fn model_agent_event(
        &mut self,
        event: AgentEvent,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        match event {
            AgentEvent::ModelRequestStart {
                invocation_id,
                request,
            } => self.model_started(invocation_id, request, occurred_at_ms, elapsed_us),
            AgentEvent::ModelProviderRequest {
                invocation_id,
                payload,
            } => TraceEntry::new("model", "provider_request", occurred_at_ms, elapsed_us)
                .correlation(invocation_id)
                .payload(&payload),
            AgentEvent::ModelProviderResponse {
                invocation_id,
                status,
                headers,
            } => TraceEntry::new("model", "provider_response", occurred_at_ms, elapsed_us)
                .correlation(invocation_id)
                .status(Some(&status.to_string()))
                .payload(&json!({ "status": status, "headers": headers })),
            AgentEvent::ModelRequestChunk {
                invocation_id,
                content_index,
                delta,
            } => self.model_chunk(
                invocation_id,
                content_index,
                &delta,
                occurred_at_ms,
                elapsed_us,
            ),
            AgentEvent::ModelRequestEnd {
                invocation_id,
                response,
            } => self.model_finished(invocation_id, &response, occurred_at_ms, elapsed_us),
            AgentEvent::ModelRequestFailed {
                invocation_id,
                code,
                message,
                outcome_unknown,
                diagnostic,
            } => self.model_failed(
                invocation_id,
                code,
                &json!({
                    "code": code,
                    "message": message,
                    "outcome_unknown": outcome_unknown,
                    "diagnostic": diagnostic
                }),
                occurred_at_ms,
                elapsed_us,
            ),
            AgentEvent::ModelRetryAttempt {
                invocation_id,
                attempt,
                next_attempt,
                category,
                delay_ms,
                cause_code,
            } => Self::model_retry(
                invocation_id,
                &json!({
                    "attempt": attempt,
                    "next_attempt": next_attempt,
                    "category": category,
                    "delay_ms": delay_ms,
                    "cause_code": cause_code
                }),
                occurred_at_ms,
                elapsed_us,
            ),
            _ => unreachable!("non-model events are dispatched by agent_event"),
        }
    }

    fn model_started(
        &mut self,
        invocation_id: String,
        request: ModelRequest,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        self.model_timings.insert(
            invocation_id.clone(),
            ModelTiming {
                started_us: elapsed_us,
                first_output_us: None,
            },
        );
        TraceEntry::new("model", "request_started", occurred_at_ms, elapsed_us)
            .correlation(invocation_id)
            .status(Some("running"))
            .payload(&to_value(request))
    }

    fn model_chunk(
        &mut self,
        invocation_id: String,
        content_index: usize,
        delta: &AssistantDelta,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        if let Some(timing) = self.model_timings.get_mut(&invocation_id)
            && timing.first_output_us.is_none()
        {
            timing.first_output_us = Some(elapsed_us);
        }
        TraceEntry::new("model", "stream_chunk", occurred_at_ms, elapsed_us)
            .correlation(invocation_id)
            .payload(&json!({ "content_index": content_index, "delta": delta }))
    }

    fn model_finished(
        &mut self,
        invocation_id: String,
        response: &ModelResponse,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        let timing = self.model_timings.remove(&invocation_id);
        let mut entry = TraceEntry::new("model", "request_finished", occurred_at_ms, elapsed_us)
            .correlation(invocation_id)
            .status(Some("completed"))
            .duration(timing.as_ref().map(|timing| elapsed_us - timing.started_us))
            .first_output(timing.and_then(|timing| {
                timing
                    .first_output_us
                    .map(|first| first - timing.started_us)
            }))
            .payload(&to_value(response));
        if let Some(usage) = response.usage {
            entry = entry.usage(
                usage.input,
                usage.output,
                usage.cache_read,
                usage.cache_write,
            );
        }
        entry
    }

    fn model_failed(
        &mut self,
        invocation_id: String,
        code: ModelFailureCode,
        payload: &Value,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        let timing = self.model_timings.remove(&invocation_id);
        TraceEntry::new("model", "request_failed", occurred_at_ms, elapsed_us)
            .correlation(invocation_id)
            .name(model_failure_name(code))
            .status(Some("failed"))
            .duration(timing.as_ref().map(|timing| elapsed_us - timing.started_us))
            .first_output(timing.and_then(|timing| {
                timing
                    .first_output_us
                    .map(|first| first - timing.started_us)
            }))
            .payload(payload)
    }

    fn model_retry(
        invocation_id: String,
        payload: &Value,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> TraceEntry {
        TraceEntry::new("model", "retry_attempt", occurred_at_ms, elapsed_us)
            .correlation(invocation_id)
            .payload(payload)
    }

    fn tool_finished(
        &mut self,
        call: ToolCall,
        occurred_at_ms: i64,
        elapsed_us: i64,
        result: renoa_agent::ToolResult,
    ) -> TraceEntry {
        let started = self.tool_starts.remove(&call.id);
        let status = if result.is_error {
            "failed"
        } else {
            "completed"
        };
        TraceEntry::new("tool", "execution_finished", occurred_at_ms, elapsed_us)
            .correlation(call.id)
            .name(call.name)
            .status(Some(status))
            .duration(started.map(|started| elapsed_us - started))
            .payload(&to_value(result))
    }

    fn tool_unknown(
        &mut self,
        call: ToolCall,
        occurred_at_ms: i64,
        elapsed_us: i64,
        error: renoa_agent::ToolOutcomeUnknown,
    ) -> TraceEntry {
        let started = self.tool_starts.remove(&call.id);
        TraceEntry::new("tool", "outcome_unknown", occurred_at_ms, elapsed_us)
            .correlation(call.id)
            .name(call.name)
            .status(Some("unknown"))
            .duration(started.map(|started| elapsed_us - started))
            .payload(&to_value(error))
    }

    fn tool_started(&mut self, call: ToolCall, occurred_at_ms: i64, elapsed_us: i64) -> TraceEntry {
        self.tool_starts.insert(call.id.clone(), elapsed_us);
        TraceEntry::new("tool", "execution_started", occurred_at_ms, elapsed_us)
            .correlation(call.id)
            .name(call.name)
            .status(Some("running"))
            .payload(&to_value(call.arguments))
    }
}

pub(super) fn now_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn lifecycle(kind: &str, occurred_at_ms: i64, elapsed_us: i64) -> TraceEntry {
    TraceEntry::new("agent", kind, occurred_at_ms, elapsed_us)
}

const fn role_name(role: renoa_agent::MessageRole) -> &'static str {
    match role {
        renoa_agent::MessageRole::User => "user",
        renoa_agent::MessageRole::Assistant => "assistant",
        renoa_agent::MessageRole::Tool => "tool",
    }
}

const fn model_failure_name(code: ModelFailureCode) -> &'static str {
    match code {
        ModelFailureCode::Authentication => "authentication",
        ModelFailureCode::RateLimited => "rate_limited",
        ModelFailureCode::InvalidRequest => "invalid_request",
        ModelFailureCode::ContextWindowExceeded => "context_window_exceeded",
        ModelFailureCode::Network => "network",
        ModelFailureCode::Timeout => "timeout",
        ModelFailureCode::ProviderUnavailable => "provider_unavailable",
        ModelFailureCode::Protocol => "protocol",
        ModelFailureCode::StreamInterrupted => "stream_interrupted",
        ModelFailureCode::Cancelled => "cancelled",
        ModelFailureCode::IncompleteStream => "incomplete_stream",
        _ => "unknown",
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_else(|error| {
        json!({
            "trace_encoding_error": error.to_string()
        })
    })
}
