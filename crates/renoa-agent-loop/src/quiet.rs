//! A reminder to say how the work is going.
//!
//! After [`QUIET_STEPS`] model steps in a row that called tools without a word
//! of text, the last tool result of the step carries [`REMINDER`] in the model
//! request; the journal never holds it. It is added to the request's final
//! messages, after the context strategy compacts and projects them, so where a
//! reminder goes depends only on the messages before it in that request: later
//! requests show it in the same place until compaction or a receipt rewrites
//! the history before it. A user message, which starts each operation and each
//! steer, counts the steps from zero.

use renoa_agent::{AssistantContent, ContentBlock, Message};

/// Steps in a row that call tools without text before the reminder. One such
/// step is ordinary; three in a row is work the person cannot see.
pub(crate) const QUIET_STEPS: usize = 3;

pub(crate) const REMINDER: &str = "<reminder>You have taken several steps without a word to \
whoever you are working for. Before your next tool call, tell them in a sentence or two what \
you have done and what comes next, then carry on.</reminder>";

/// Adds [`REMINDER`] to the last tool result of every [`QUIET_STEPS`]th quiet
/// step in `messages`.
pub(crate) fn remind(messages: &mut [Message]) {
    let mut quiet = 0;
    for index in 0..messages.len() {
        let ends_step = !matches!(messages.get(index + 1), Some(Message::Tool { .. }));
        match &mut messages[index] {
            Message::User { .. } => quiet = 0,
            Message::Assistant { content, .. } => {
                if spoke(content) {
                    quiet = 0;
                } else if content
                    .iter()
                    .any(|block| matches!(block, AssistantContent::ToolCall { .. }))
                {
                    quiet += 1;
                }
            }
            Message::Tool { result } => {
                if ends_step && quiet >= QUIET_STEPS {
                    result.content.push(ContentBlock::text(REMINDER));
                    quiet = 0;
                }
            }
        }
    }
}

fn spoke(content: &[AssistantContent]) -> bool {
    content.iter().any(
        |block| matches!(block, AssistantContent::Text { text, .. } if !text.trim().is_empty()),
    )
}

#[cfg(test)]
mod tests {
    use renoa_agent::{
        AssistantContent, AssistantMetadata, ContentBlock, Message, StopReason, ToolCall,
        ToolResult,
    };
    use serde_json::json;

    use super::{QUIET_STEPS, REMINDER, remind};

    fn step(text: Option<&str>, calls: usize) -> Vec<Message> {
        let mut content: Vec<AssistantContent> = text
            .map(|text| AssistantContent::Text {
                text: text.to_owned(),
                signature: None,
            })
            .into_iter()
            .collect();
        let mut results = Vec::new();
        for call in 0..calls {
            let id = format!("call-{call}");
            content.push(AssistantContent::ToolCall {
                call: ToolCall {
                    id: id.clone(),
                    name: "bash".to_owned(),
                    arguments: json!({}),
                    thought_signature: None,
                    namespace: None,
                },
            });
            results.push(Message::Tool {
                result: ToolResult {
                    call_id: id,
                    name: "bash".to_owned(),
                    content: vec![ContentBlock::text("ok")],
                    details: None,
                    is_error: false,
                    closing: None,
                },
            });
        }
        let mut messages = vec![Message::Assistant {
            content,
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        }];
        messages.extend(results);
        messages
    }

    fn reminded(messages: &[Message]) -> Vec<usize> {
        messages
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                matches!(message, Message::Tool { result }
                    if result.content.contains(&ContentBlock::text(REMINDER)))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn user(text: &str) -> Message {
        Message::User {
            content: vec![ContentBlock::text(text)],
        }
    }

    #[test]
    fn the_last_result_of_every_third_quiet_step_carries_the_reminder() {
        let mut messages = vec![user("Render the reel.")];
        for _ in 0..QUIET_STEPS - 1 {
            messages.extend(step(None, 1));
        }
        // The step that reaches the count calls two tools: only its last
        // result carries the reminder.
        messages.extend(step(None, 2));
        let mut shown = messages.clone();
        remind(&mut shown);
        let first = messages.len() - 1;
        assert_eq!(reminded(&shown), vec![first]);

        // Later requests keep it in place, and count again from it.
        for _ in 0..QUIET_STEPS {
            messages.extend(step(None, 1));
        }
        let mut later = messages.clone();
        remind(&mut later);
        assert_eq!(reminded(&later), vec![first, messages.len() - 1]);
    }

    #[test]
    fn a_word_to_the_person_or_a_new_message_counts_again() {
        let mut messages = vec![user("Render the reel.")];
        messages.extend(step(None, 1));
        messages.extend(step(None, 1));
        messages.extend(step(Some("Clips are rendering on Colab."), 1));
        messages.extend(step(None, 1));
        messages.push(user("Make it shorter."));
        messages.extend(step(None, 1));
        messages.extend(step(None, 1));
        messages.extend(step(Some("   "), 1));

        let mut shown = messages.clone();
        remind(&mut shown);
        assert_eq!(
            reminded(&shown),
            vec![messages.len() - 1],
            "only blank text after the steer leaves three quiet steps"
        );
    }
}
