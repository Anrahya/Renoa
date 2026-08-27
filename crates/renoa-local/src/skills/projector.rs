use std::collections::HashSet;

use renoa_agent::{ContentBlock, Message};
use renoa_agent_loop::{ContextProjector, ContextStrategyError};
use serde_json::Value;

use super::tool::{ACTIVATION_DETAIL_KIND, SKILL_LOAD_TOOL};

pub(crate) struct ActivatedSkillProjector {
    references: HashSet<String>,
}

impl ActivatedSkillProjector {
    pub(crate) fn new(references: HashSet<String>) -> Self {
        Self { references }
    }
}

impl ContextProjector for ActivatedSkillProjector {
    fn project(&self, mut messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError> {
        for message in &mut messages {
            let Message::Tool { result } = message else {
                continue;
            };
            if result.name != SKILL_LOAD_TOOL || result.is_error {
                continue;
            }
            let Some(reference) = activation_reference(result.details.as_ref()) else {
                continue;
            };
            if !self.references.contains(reference) {
                continue;
            }
            result.content = vec![ContentBlock::text(format!(
                "Skill {reference} remains active; its exact instructions are reattached above."
            ))];
        }
        Ok(messages)
    }
}

fn activation_reference(details: Option<&Value>) -> Option<&str> {
    let details = details?.as_object()?;
    if details.get("kind")?.as_str()? != ACTIVATION_DETAIL_KIND {
        return None;
    }
    details.get("reference")?.as_str()
}

#[cfg(test)]
mod tests {
    use renoa_agent::{ContentBlock, Message, ToolResult};
    use renoa_agent_loop::ContextProjector as _;
    use serde_json::json;

    use super::{ACTIVATION_DETAIL_KIND, ActivatedSkillProjector};

    #[test]
    fn only_previously_active_skill_results_are_compacted_to_receipts() {
        let active =
            "skill:review:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let new = "skill:test:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let projector = ActivatedSkillProjector::new([active.to_owned()].into());
        let messages = vec![result("one", active), result("two", new)];

        let projected = projector.project(messages).expect("project context");

        let Message::Tool { result: first } = &projected[0] else {
            panic!("first message is not a tool result");
        };
        assert_eq!(
            first.content,
            [ContentBlock::text(format!(
                "Skill {active} remains active; its exact instructions are reattached above."
            ))]
        );
        let Message::Tool { result: second } = &projected[1] else {
            panic!("second message is not a tool result");
        };
        assert_eq!(second.content, [ContentBlock::text("full instructions")]);
    }

    fn result(call_id: &str, reference: &str) -> Message {
        Message::Tool {
            result: ToolResult {
                call_id: call_id.to_owned(),
                name: "skill_load".to_owned(),
                content: vec![ContentBlock::text("full instructions")],
                details: Some(json!({
                    "kind": ACTIVATION_DETAIL_KIND,
                    "reference": reference,
                })),
                is_error: false,
            },
        }
    }
}
