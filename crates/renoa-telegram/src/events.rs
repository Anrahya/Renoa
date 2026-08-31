use std::sync::{Arc, Mutex};

use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantDelta, BoxFuture, ModelFailureCode, StopReason,
};
use tokio_util::sync::CancellationToken;

use crate::{api::TelegramApi, ingress::Topic, log};

const DRAFT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const DRAFT_REFRESH_TICKS: u64 = 10;
const MAX_DRAFT_UTF16_UNITS: usize = 4096;
const MAX_ACCUMULATED_PROGRESS_UTF16_UNITS: usize = MAX_DRAFT_UTF16_UNITS * 2;

pub(crate) struct SurfaceEvents {
    state: Arc<Mutex<ProgressState>>,
}

struct ProgressState {
    progress: String,
    visible: String,
    status: String,
    revision: u64,
}

struct DraftSnapshot {
    thinking: Option<String>,
    text: Option<String>,
}

impl ProgressState {
    fn new() -> Self {
        Self {
            progress: String::new(),
            visible: String::new(),
            status: "Thinking…".to_owned(),
            revision: 1,
        }
    }
}

impl SurfaceEvents {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ProgressState::new())),
        }
    }

    pub(crate) fn start_drafts(
        &self,
        api: Arc<TelegramApi>,
        topic: Topic,
        draft_id: i64,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(DRAFT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut sent_revision = 0;
            let mut ticks_since_send = DRAFT_REFRESH_TICKS;
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    _ = interval.tick() => {
                        ticks_since_send = ticks_since_send.saturating_add(1);
                        let Some((revision, draft)) = snapshot(&state) else {
                            log::event("error", "progress_state_poisoned", &serde_json::json!({"draft_id": draft_id}));
                            return;
                        };
                        if revision == sent_revision && ticks_since_send < DRAFT_REFRESH_TICKS {
                            continue;
                        }
                        let delivery = tokio::select! {
                            () = shutdown.cancelled() => return,
                            result = api.send_draft(
                                topic.chat_id,
                                topic.thread_id,
                                draft_id,
                                draft.thinking.as_deref(),
                                draft.text.as_deref(),
                            ) => result,
                        };
                        match delivery {
                            Ok(()) => {
                                sent_revision = revision;
                                ticks_since_send = 0;
                            }
                            Err(error) => log::event(
                                "warn",
                                "draft_delivery_failed",
                                &serde_json::json!({"draft_id": draft_id, "error": error.to_string()}),
                            ),
                        }
                    }
                }
            }
        })
    }

    fn update(&self, action: impl FnOnce(&mut ProgressState) -> bool) {
        if let Ok(mut state) = self.state.lock()
            && action(&mut state)
        {
            state.revision = state.revision.saturating_add(1);
        }
    }
}

impl AgentEventSink for SurfaceEvents {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        self.update(|state| observe(state, event));
        Box::pin(std::future::ready(()))
    }
}

fn observe(state: &mut ProgressState, event: AgentEvent) -> bool {
    match event {
        AgentEvent::ModelRequestStart { .. } => set_status(state, "Thinking…"),
        AgentEvent::MessageAbort => {
            let cleared = clear_visible(state);
            set_status(state, "Retrying…") || cleared
        }
        AgentEvent::MessageUpdate {
            delta: AssistantDelta::Text { text },
            ..
        } => {
            if text.is_empty() {
                return false;
            }
            state.visible.push_str(&text);
            state.status.clear();
            true
        }
        AgentEvent::ModelRequestEnd { response, .. } => match response.stop_reason {
            StopReason::ToolUse => archive_visible(state),
            StopReason::Stop | StopReason::Length => set_status(state, ""),
        },
        AgentEvent::ToolExecutionStart { call } => {
            set_status(state, &format!("Using {}…", tool_label(&call.name)))
        }
        AgentEvent::ToolExecutionEnd { call, result } => {
            let label = tool_label(&call.name);
            let summary = if result.is_error {
                format!("{label} failed.")
            } else {
                format!("Finished {label}.")
            };
            let appended = append_progress(state, &summary);
            set_status(state, "Thinking…") || appended
        }
        AgentEvent::ToolExecutionOutcomeUnknown { call, .. } => {
            let appended = append_progress(
                state,
                &format!("The outcome of {} is unknown.", tool_label(&call.name)),
            );
            set_status(state, "Checking what happened…") || appended
        }
        AgentEvent::ModelRetryAttempt {
            next_attempt,
            delay_ms,
            ..
        } => set_status(
            state,
            &format!("Retrying model call {next_attempt} in {delay_ms} ms…"),
        ),
        AgentEvent::ModelRequestFailed { code, .. } => set_status(
            state,
            if matches!(code, ModelFailureCode::Cancelled) {
                "Stopping…"
            } else {
                "Model call failed…"
            },
        ),
        AgentEvent::MessageUpdate {
            delta:
                AssistantDelta::Reasoning { .. }
                | AssistantDelta::ToolCallStart { .. }
                | AssistantDelta::ToolCallArguments { .. },
            ..
        }
        | AgentEvent::MessageStart { .. }
        | AgentEvent::ModelProviderRequest { .. }
        | AgentEvent::ModelProviderResponse { .. }
        | AgentEvent::ModelRequestChunk { .. }
        | AgentEvent::ToolExecutionUpdate { .. } => false,
    }
}

fn set_status(state: &mut ProgressState, status: &str) -> bool {
    if state.status == status {
        return false;
    }
    state.status.clear();
    state.status.push_str(status);
    true
}

fn clear_visible(state: &mut ProgressState) -> bool {
    if state.visible.is_empty() {
        return false;
    }
    state.visible.clear();
    true
}

fn archive_visible(state: &mut ProgressState) -> bool {
    let visible = std::mem::take(&mut state.visible);
    append_progress(state, visible.trim())
}

fn append_progress(state: &mut ProgressState, text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if !state.progress.is_empty() {
        state.progress.push_str("\n\n");
    }
    state.progress.push_str(text);
    state.progress = bounded_tail(&state.progress, MAX_ACCUMULATED_PROGRESS_UTF16_UNITS);
    true
}

fn tool_label(name: &str) -> String {
    name.replace('_', " ")
}

fn snapshot(state: &Mutex<ProgressState>) -> Option<(u64, DraftSnapshot)> {
    let state = state.lock().ok()?;
    Some((state.revision, render_draft(&state)))
}

fn render_draft(state: &ProgressState) -> DraftSnapshot {
    let mut thinking = state.progress.clone();
    if !state.status.is_empty() {
        if !thinking.is_empty() {
            thinking.push_str("\n\n");
        }
        thinking.push_str(&state.status);
    }

    let text =
        (!state.visible.is_empty()).then(|| bounded_tail(&state.visible, MAX_DRAFT_UTF16_UNITS));
    let text_units = text
        .as_deref()
        .map_or(0, |value| value.encode_utf16().count());
    let thinking_budget = MAX_DRAFT_UTF16_UNITS.saturating_sub(text_units);
    let thinking = (!thinking.is_empty() && thinking_budget > 0)
        .then(|| bounded_tail(&thinking, thinking_budget));

    DraftSnapshot { thinking, text }
}

fn bounded_tail(text: &str, max_utf16_units: usize) -> String {
    if max_utf16_units == 0 {
        return String::new();
    }
    if text.encode_utf16().count() <= max_utf16_units {
        return text.to_owned();
    }
    let mut kept_units = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let next = kept_units + character.len_utf16();
        if next >= max_utf16_units {
            break;
        }
        kept_units = next;
        start = index;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use renoa_agent::{
        AgentEvent, AssistantDelta, AssistantMetadata, ContentBlock, ModelResponse, StopReason,
        ToolCall, ToolResult,
    };

    use super::{MAX_DRAFT_UTF16_UNITS, ProgressState, bounded_tail, observe, render_draft};

    #[test]
    fn progress_survives_tool_round_trips_until_the_final_answer() {
        let mut state = ProgressState::new();
        assert!(observe(
            &mut state,
            AgentEvent::MessageUpdate {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Checking the extension catalog.".to_owned(),
                },
            },
        ));
        assert!(observe(&mut state, completed(StopReason::ToolUse),));
        assert!(observe(
            &mut state,
            AgentEvent::ToolExecutionStart {
                call: tool_call("extension_manage"),
            },
        ));
        let using = render_draft(&state);
        assert_eq!(
            using.thinking.as_deref(),
            Some("Checking the extension catalog.\n\nUsing extension manage…")
        );
        assert_eq!(using.text, None);

        assert!(observe(
            &mut state,
            AgentEvent::ToolExecutionEnd {
                call: tool_call("extension_manage"),
                result: ToolResult {
                    call_id: "call-1".to_owned(),
                    name: "extension_manage".to_owned(),
                    content: vec![ContentBlock::text("done")],
                    details: None,
                    is_error: false,
                },
            },
        ));
        assert!(!observe(
            &mut state,
            AgentEvent::ModelRequestStart {
                invocation_id: "model-2".to_owned(),
                request: renoa_agent::ModelRequest {
                    system_prompt: String::new(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                },
            },
        ));
        assert!(observe(
            &mut state,
            AgentEvent::MessageUpdate {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Found Exa.".to_owned(),
                },
            },
        ));
        let final_stream = render_draft(&state);
        assert_eq!(
            final_stream.thinking.as_deref(),
            Some("Checking the extension catalog.\n\nFinished extension manage.")
        );
        assert_eq!(final_stream.text.as_deref(), Some("Found Exa."));
    }

    #[test]
    fn hidden_reasoning_does_not_replace_progress_or_trigger_a_draft() {
        let mut state = ProgressState::new();
        assert!(!observe(
            &mut state,
            AgentEvent::MessageUpdate {
                content_index: 0,
                delta: AssistantDelta::Reasoning {
                    text: "private chain of thought".to_owned(),
                },
            },
        ));
        assert!(state.progress.is_empty());
        assert!(state.visible.is_empty());
        assert_eq!(render_draft(&state).thinking.as_deref(), Some("Thinking…"));
    }

    #[test]
    fn long_utf8_drafts_keep_a_valid_bounded_tail() {
        let input = format!("prefix{}", "🦀".repeat(MAX_DRAFT_UTF16_UNITS));
        let output = bounded_tail(&input, MAX_DRAFT_UTF16_UNITS);
        assert!(output.encode_utf16().count() <= MAX_DRAFT_UTF16_UNITS);
        assert!(output.starts_with('…'));
        assert!(output.ends_with('🦀'));
    }

    #[test]
    fn drafts_at_the_utf16_limit_are_unchanged() {
        let input = "🦀".repeat(MAX_DRAFT_UTF16_UNITS / 2);
        assert_eq!(bounded_tail(&input, MAX_DRAFT_UTF16_UNITS), input);
    }

    fn completed(stop_reason: StopReason) -> AgentEvent {
        AgentEvent::ModelRequestEnd {
            invocation_id: "model-1".to_owned(),
            response: ModelResponse {
                content: Vec::new(),
                stop_reason,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_owned(),
            name: name.to_owned(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        }
    }
}
