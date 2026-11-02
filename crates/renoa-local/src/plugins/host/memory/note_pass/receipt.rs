//! What a note pass leaves in its conversation: one receipt naming the notes
//! it wrote, in place of the pass's own exchange, in every later request. The
//! journal keeps the exchange; the receipt is rendered from it, so a retried
//! or recovered command sees the same one.

use std::sync::Arc;

use renoa_agent::{AssistantContent, Message};
use renoa_agent_loop::{ContextProjector, ContextStrategyError};

use super::super::tools::NOTE_DETAIL_KIND;

/// The receipt rendering's revision, part of the context binding's.
pub(crate) const REVISION: &str = "renoa.memory.pass-receipt.v1";

/// The ids of the notes `messages` wrote, in order: each `memory_note` result
/// that succeeded. The pass runs `memory_note` only when called directly (see
/// `guard`), so every note it wrote is here.
pub(crate) fn noted(messages: &[Message]) -> Vec<u64> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { result } if !result.is_error => {
                let details = result.details.as_ref()?;
                (details.get("kind")?.as_str()? == NOTE_DETAIL_KIND)
                    .then(|| details.get("id")?.as_u64())
                    .flatten()
            }
            _ => None,
        })
        .collect()
}

/// The tool calls in `messages` that returned an error.
pub(crate) fn failed_calls(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message, Message::Tool { result } if result.is_error))
        .count()
}

/// The receipt of a pass whose messages are `messages`. A pass that did not
/// reach its final answer says it was interrupted.
pub(crate) fn receipt(messages: &[Message]) -> String {
    let finished = matches!(
        messages.last(),
        Some(Message::Assistant { content, .. })
            if !content.iter().any(|block| matches!(block, AssistantContent::ToolCall { .. }))
    );
    let lead = if finished {
        "memory pass"
    } else {
        "memory pass interrupted"
    };
    let ids: Vec<String> = noted(messages).iter().map(|id| format!("#{id}")).collect();
    if ids.is_empty() {
        format!("{lead}: nothing new")
    } else {
        format!("{lead}: noted {}", ids.join(", "))
    }
}

/// The Host's message projector: the skill projector, when the agent has
/// active skills, and the note pass's receipts.
pub(crate) struct ReceiptProjector {
    skills: Option<Arc<dyn ContextProjector>>,
}

impl ReceiptProjector {
    pub(crate) fn new(skills: Option<Arc<dyn ContextProjector>>) -> Self {
        Self { skills }
    }
}

impl ContextProjector for ReceiptProjector {
    fn project(&self, messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError> {
        match &self.skills {
            Some(skills) => skills.project(messages),
            None => Ok(messages),
        }
    }

    fn receipt(&self, messages: &[Message]) -> Result<String, ContextStrategyError> {
        Ok(receipt(messages))
    }
}

#[cfg(test)]
mod tests {
    use renoa_agent::{AssistantContent, Message, StopReason, ToolCall, ToolResult};
    use serde_json::json;

    use super::{NOTE_DETAIL_KIND, receipt};

    fn result(name: &str, details: Option<serde_json::Value>, is_error: bool) -> Message {
        Message::Tool {
            result: ToolResult {
                call_id: "call".to_owned(),
                name: name.to_owned(),
                content: Vec::new(),
                details,
                is_error,
            },
        }
    }

    fn answer(content: Vec<AssistantContent>) -> Message {
        Message::Assistant {
            content,
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: renoa_agent::AssistantMetadata::default(),
        }
    }

    #[test]
    fn a_receipt_names_the_notes_written_and_whether_the_pass_finished() {
        let note = |id: u64| Some(json!({"kind": NOTE_DETAIL_KIND, "id": id}));
        let call = AssistantContent::tool_call(ToolCall {
            id: "call".to_owned(),
            name: "memory_note".to_owned(),
            arguments: json!({}),
            thought_signature: None,
            namespace: None,
        });
        let pass = vec![
            Message::user_text("Memory pass."),
            answer(vec![call.clone()]),
            result("memory_note", note(41), false),
            result("tool_execute", note(42), false),
            result("memory_note", note(43), true),
            result(
                "memory_recall",
                Some(json!({"kind": "other", "id": 7})),
                false,
            ),
            answer(vec![AssistantContent::text("#41, #42")]),
        ];
        assert_eq!(receipt(&pass), "memory pass: noted #41, #42");
        assert_eq!(
            receipt(&[
                Message::user_text("Memory pass."),
                answer(vec![AssistantContent::text("NONE")])
            ]),
            "memory pass: nothing new"
        );
        assert_eq!(
            receipt(&pass[..4]),
            "memory pass interrupted: noted #41, #42",
            "a pass stopped before its answer"
        );
        assert_eq!(
            receipt(&[Message::user_text("Memory pass."), answer(vec![call])]),
            "memory pass interrupted: nothing new"
        );
    }
}
