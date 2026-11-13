use std::time::{Duration, Instant};

use renoa_agent::{AgentEvent, AssistantDelta, ModelRequest, ToolSpec};
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
    text.request_started();
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
fn abort_after_summarized_activity_without_text_emits_reset() {
    let start = Instant::now();
    let mut live = LiveText::default();
    live.observe(&request(true), start);

    assert_eq!(
        live.observe(
            &update(AssistantDelta::ReasoningSummary {
                text: "**Planning**".to_owned(),
            }),
            start,
        ),
        Some(LiveActivity::Activity {
            text: "Planning".to_owned(),
        })
    );
    assert_eq!(
        live.observe(&AgentEvent::MessageAbort, start),
        Some(LiveActivity::TextReset)
    );
    assert_eq!(
        live.observe(&AgentEvent::MessageAbort, start),
        None,
        "reset clears activity state so repeated abort is silent"
    );
}

#[test]
fn text_streams_only_while_a_request_is_in_flight() {
    let start = Instant::now();
    let mut text = LiveText::default();
    assert_eq!(text.push("early", start), None, "no request has started");

    text.request_started();
    assert_eq!(text.push("Hello", start), Some(delta(0, "Hello")));
    text.request_ended();
    assert_eq!(text.push("late", start + FLUSH_INTERVAL), None);
}

#[test]
fn a_request_that_offers_no_tools_streams_its_text() {
    let start = Instant::now();
    let mut live = LiveText::default();

    assert_eq!(live.observe(&request(false), start), None);
    assert_eq!(
        live.observe(&update(text("Hello")), start),
        Some(delta(0, "Hello")),
        "only the summary binding keeps a summary off the wire"
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

#[test]
fn summarized_reasoning_emits_bounded_first_line_of_latest_summary() {
    let start = Instant::now();
    let mut live = LiveText::default();
    live.observe(&request(true), start);

    let summary = |part: &str| {
        update(AssistantDelta::ReasoningSummary {
            text: part.to_owned(),
        })
    };

    // First summary emits its heading cleanly without markdown framing.
    assert_eq!(
        live.observe(
            &summary("**Inspecting `Cargo.toml`**\nMore detail line"),
            start
        ),
        Some(LiveActivity::Activity {
            text: "Inspecting Cargo.toml".to_owned()
        })
    );
    // Appending more body paragraphs does not replace the heading with body prose.
    assert_eq!(
        live.observe(
            &summary("\n\nBody detail paragraph"),
            start + FLUSH_INTERVAL
        ),
        None
    );
    // A new summary heading replaces the earlier one and fits long strings cleanly.
    let long_heading = format!("\n\n### {}", "a".repeat(200));
    assert_eq!(
        live.observe(&summary(&long_heading), start + FLUSH_INTERVAL * 2),
        Some(LiveActivity::Activity {
            text: "a".repeat(renoa_protocol::MAX_ACTIVITY_BYTES)
        })
    );
    // Visible text clears reasoning and streams normally.
    assert_eq!(
        live.observe(&update(text("Done")), start + FLUSH_INTERVAL * 3),
        Some(delta(0, "Done"))
    );
}

#[test]
fn summarized_reasoning_prefers_latest_title_cuts_on_word_boundary_and_survives_long_body() {
    let start = Instant::now();
    let mut live = LiveText::default();
    live.observe(&request(true), start);

    let summary = |part: &str| {
        update(AssistantDelta::ReasoningSummary {
            text: part.to_owned(),
        })
    };

    assert_eq!(
        live.observe(
            &summary("**Planning**\n\nInspecting workspace files\n\nChecking details"),
            start
        ),
        Some(LiveActivity::Activity {
            text: "Planning".to_owned()
        })
    );

    let padding = format!("\n\n{}", "word ".repeat(8_000));
    assert_eq!(
        live.observe(&summary(&padding), start + FLUSH_INTERVAL),
        None
    );
    assert_eq!(
        live.observe(
            &summary("\n\nTrailing prose paragraph"),
            start + FLUSH_INTERVAL * 2
        ),
        None,
        "heading must remain active even after body exceeds retention cap"
    );

    let wordy = "**Checking every dependency across renoa-protocol and renoa-agent and renoa-node workspace crates for compatibility issues and lockfiles**\n\nDetails";
    let frame = live.observe(
        &summary(&format!("\n\n{wordy}")),
        start + FLUSH_INTERVAL * 3,
    );
    assert_eq!(
        frame,
        Some(LiveActivity::Activity {
            text: "Checking every dependency across renoa-protocol and renoa-agent and renoa-node workspace crates for compatibility issues".to_owned(),
        })
    );
}

#[test]
fn oversized_reasoning_paragraph_followed_by_separator_does_not_wedge_next_summary() {
    let start = Instant::now();
    let mut live = LiveText::default();
    live.observe(&request(true), start);

    let summary = |part: &str| {
        update(AssistantDelta::ReasoningSummary {
            text: part.to_owned(),
        })
    };

    let huge_block = "x".repeat(super::MAX_REASONING_BUFFER_BYTES + 1024);
    assert!(matches!(
        live.observe(&summary(&huge_block), start),
        Some(LiveActivity::Activity { .. })
    ));
    assert_eq!(live.observe(&summary("\n\n"), start + FLUSH_INTERVAL), None);
    assert_eq!(
        live.observe(&summary("Next step\n"), start + FLUSH_INTERVAL * 2),
        Some(LiveActivity::Activity {
            text: "Next step".to_owned()
        })
    );
}
