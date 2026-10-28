//! The ephemeral text a running turn streams to attached surfaces.
//!
//! Only visible assistant text leaves the node: reasoning and tool-call
//! fragments never do. Text is coalesced into at most one frame per
//! [`FLUSH_INTERVAL`]. Text that arrives inside the interval waits for the
//! message's next fragment of any kind once the interval has passed (the
//! fragment itself stays on the node), so a tool call that streams for seconds
//! after a sentence does not hold the sentence's tail back. A model that
//! stalls sends nothing until its next fragment. Whatever is still pending
//! when a message ends is dropped, because the turn's durable
//! `assistant_message` supersedes the live text. A message's frames count from
//! zero, so a surface can tell that it missed one.

use std::time::{Duration, Instant};

use renoa_agent::{AgentEvent, AssistantDelta};
use renoa_protocol::LiveActivity;

/// The least time between two text frames of one message.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub(crate) struct LiveText {
    /// Whether the model request in flight may stream its text.
    streaming: bool,
    next_sequence: u64,
    pending: String,
    last_sent: Option<Instant>,
}

impl LiveText {
    /// The live frame one agent event completes, if any. Only visible text of
    /// a model request that offers tools leaves the node: reasoning and
    /// tool-call fragments never do.
    pub(crate) fn observe(&mut self, event: &AgentEvent, now: Instant) -> Option<LiveActivity> {
        match event {
            AgentEvent::ModelRequestStart { request, .. } => {
                self.request_started(!request.tools.is_empty());
                None
            }
            AgentEvent::MessageStart { .. } => {
                self.message_started();
                None
            }
            AgentEvent::MessageUpdate {
                delta: AssistantDelta::Text { text },
                ..
            } => self.push(text, now),
            // Reasoning and tool-call fragments are not sent. They only show
            // that the visible text before them is complete for now.
            AgentEvent::MessageUpdate { .. } => self.push("", now),
            AgentEvent::MessageAbort => self.abort(),
            AgentEvent::ModelRequestEnd { .. } | AgentEvent::ModelRequestFailed { .. } => {
                self.request_ended();
                None
            }
            _ => None,
        }
    }

    /// A model request starts. A request that offers no tools is a context
    /// compaction summary, which is internal and never streamed: the loop
    /// refuses a summary request that advertises tools
    /// (`renoa-agent-loop`, `CompactionPlan::new`).
    fn request_started(&mut self, offers_tools: bool) {
        self.streaming = offers_tools;
        self.message_started();
    }

    /// The request ended, completed or not.
    fn request_ended(&mut self) {
        self.streaming = false;
        self.message_started();
    }

    /// A new message starts, so its frames count from zero.
    fn message_started(&mut self) {
        self.next_sequence = 0;
        self.pending.clear();
        self.last_sent = None;
    }

    /// One visible text delta arrived at `now`, or a fragment that is not text
    /// (empty `text`). Returns the frame that is due, carrying everything
    /// pending since the last one.
    fn push(&mut self, text: &str, now: Instant) -> Option<LiveActivity> {
        if !self.streaming {
            return None;
        }
        self.pending.push_str(text);
        if self.pending.is_empty() {
            return None;
        }
        if self
            .last_sent
            .is_some_and(|sent| now.saturating_duration_since(sent) < FLUSH_INTERVAL)
        {
            return None;
        }
        self.last_sent = Some(now);
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        Some(LiveActivity::TextDelta {
            sequence,
            text: std::mem::take(&mut self.pending),
        })
    }

    /// The message was abandoned. Returns a reset when a surface has been
    /// shown any of it.
    fn abort(&mut self) -> Option<LiveActivity> {
        let shown = self.next_sequence > 0;
        self.message_started();
        shown.then_some(LiveActivity::TextReset)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use renoa_agent::{AgentEvent, AssistantDelta, MessageRole, ModelRequest, ToolSpec};
    use renoa_protocol::LiveActivity;

    use super::{FLUSH_INTERVAL, LiveText};

    fn request(offers_tools: bool) -> AgentEvent {
        AgentEvent::ModelRequestStart {
            invocation_id: "invocation".to_owned(),
            request: ModelRequest {
                system_prompt: String::new(),
                messages: Vec::new(),
                tools: if offers_tools {
                    vec![ToolSpec {
                        name: "read_file".to_owned(),
                        description: "Reads a file.".to_owned(),
                        input_schema: serde_json::json!({}),
                    }]
                } else {
                    Vec::new()
                },
            },
        }
    }

    fn update(delta: AssistantDelta) -> AgentEvent {
        AgentEvent::MessageUpdate {
            content_index: 0,
            delta,
        }
    }

    fn text(text: &str) -> AssistantDelta {
        AssistantDelta::Text {
            text: text.to_owned(),
        }
    }

    fn delta(sequence: u64, text: &str) -> LiveActivity {
        LiveActivity::TextDelta {
            sequence,
            text: text.to_owned(),
        }
    }

    fn streaming() -> LiveText {
        let mut text = LiveText::default();
        text.request_started(true);
        text
    }

    #[test]
    fn deltas_coalesce_into_numbered_frames() {
        let start = Instant::now();
        let mut text = streaming();

        assert_eq!(text.push("Hel", start), Some(delta(0, "Hel")));
        assert_eq!(text.push("lo", start + Duration::from_millis(10)), None);
        assert_eq!(text.push(", wor", start + Duration::from_millis(20)), None);
        assert_eq!(
            text.push("ld", start + FLUSH_INTERVAL),
            Some(delta(1, "lo, world"))
        );
    }

    #[test]
    fn a_new_message_counts_from_zero_and_drops_unsent_text() {
        let start = Instant::now();
        let mut text = streaming();
        assert_eq!(text.push("one", start), Some(delta(0, "one")));
        assert_eq!(text.push("unsent", start + Duration::from_millis(1)), None);

        text.message_started();

        assert_eq!(
            text.push("two", start + Duration::from_millis(2)),
            Some(delta(0, "two"))
        );
    }

    #[test]
    fn an_abandoned_message_resets_only_what_a_surface_was_shown() {
        let start = Instant::now();
        let mut text = streaming();
        assert_eq!(text.abort(), None, "nothing was shown");

        assert_eq!(text.push("shown", start), Some(delta(0, "shown")));
        assert_eq!(text.abort(), Some(LiveActivity::TextReset));
        assert_eq!(text.push("again", start), Some(delta(0, "again")));
    }

    #[test]
    fn a_request_that_offers_no_tools_streams_nothing() {
        let start = Instant::now();
        let mut text = LiveText::default();
        text.request_started(false);

        assert_eq!(text.push("## Goal and user intent", start), None);

        text.request_started(true);
        assert_eq!(text.push("Hello", start), Some(delta(0, "Hello")));
        text.request_ended();
        assert_eq!(text.push("late", start + FLUSH_INTERVAL), None);
    }

    #[test]
    fn a_summary_request_that_offers_no_tools_never_streams_through_the_event_mapping() {
        let start = Instant::now();
        let mut live = LiveText::default();

        assert_eq!(live.observe(&request(false), start), None);
        assert_eq!(
            live.observe(
                &AgentEvent::MessageStart {
                    role: MessageRole::Assistant
                },
                start
            ),
            None
        );
        assert_eq!(
            live.observe(&update(text("## Goal and user intent")), start),
            None
        );

        assert_eq!(live.observe(&request(true), start), None);
        assert_eq!(
            live.observe(&update(text("Hello")), start),
            Some(delta(0, "Hello"))
        );
    }

    #[test]
    fn text_waiting_out_the_interval_is_sent_by_the_next_fragment_of_any_kind() {
        let start = Instant::now();
        let mut live = LiveText::default();
        live.observe(&request(true), start);
        assert_eq!(
            live.observe(&update(text("Let me ")), start),
            Some(delta(0, "Let me "))
        );
        assert_eq!(
            live.observe(&update(text("write")), start + Duration::from_millis(10)),
            None,
            "inside the interval the text waits"
        );
        let call = |json_delta: &str| {
            update(AssistantDelta::ToolCallArguments {
                json_delta: json_delta.to_owned(),
            })
        };

        assert_eq!(
            live.observe(&call("{\"path\":"), start + Duration::from_millis(20)),
            None,
            "a fragment inside the interval sends nothing"
        );
        assert_eq!(
            live.observe(&call("\"a.txt\"}"), start + FLUSH_INTERVAL),
            Some(delta(1, "write")),
            "the first fragment after the interval carries the waiting text, not its own"
        );
        assert_eq!(
            live.observe(&call("more"), start + FLUSH_INTERVAL * 3),
            None,
            "nothing is left to send"
        );
    }

    #[test]
    fn reasoning_and_tool_call_fragments_never_stream() {
        let start = Instant::now();
        let mut live = LiveText::default();
        live.observe(&request(true), start);

        for fragment in [
            AssistantDelta::Reasoning {
                text: "private deliberation".to_owned(),
            },
            AssistantDelta::ToolCallStart {
                id: "call".to_owned(),
                name: "read_file".to_owned(),
            },
            AssistantDelta::ToolCallArguments {
                json_delta: "{\"path\":".to_owned(),
            },
        ] {
            assert_eq!(live.observe(&update(fragment), start), None);
        }
        assert_eq!(
            live.observe(&update(text("visible")), start),
            Some(delta(0, "visible"))
        );
    }
}
