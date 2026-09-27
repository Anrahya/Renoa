use std::collections::BTreeMap;

use renoa_agent::{ContentBlock, Message};
use renoa_agent_loop::{ContextProjector, ContextStrategyError};
use serde_json::Value;

use super::tool::{ACTIVATION_DETAIL_KIND, SKILL_LOAD_TOOL};

pub(crate) struct ActivatedSkillProjector {
    bodies: BTreeMap<String, String>,
}

impl ActivatedSkillProjector {
    pub(crate) fn new(bodies: BTreeMap<String, String>) -> Self {
        Self { bodies }
    }
}

impl ContextProjector for ActivatedSkillProjector {
    fn project(&self, mut messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError> {
        for message in &mut messages {
            let Message::Tool { result } = message else {
                continue;
            };
            if result.name == "code_mode" && !result.is_error {
                for content in &mut result.content {
                    if let ContentBlock::Text { text } = content
                        && let Ok(mut value) = serde_json::from_str::<Value>(text)
                        && project_code_result(&mut value, &self.bodies)
                    {
                        *text = serde_json::to_string(&value)
                            .map_err(|error| ContextStrategyError::new(error.to_string()))?;
                    }
                }
                continue;
            }
            if !matches!(
                result.name.as_str(),
                SKILL_LOAD_TOOL | crate::capabilities::TOOL_EXECUTE
            ) || result.is_error
            {
                continue;
            }
            let Some(reference) = activation_reference(result.details.as_ref()) else {
                continue;
            };
            if !self.bodies.contains_key(reference) {
                continue;
            }
            result.content = vec![ContentBlock::text(format!(
                "Skill {reference} remains active; its exact instructions are reattached above."
            ))];
        }
        Ok(messages)
    }
}

fn project_code_result(value: &mut Value, bodies: &BTreeMap<String, String>) -> bool {
    if let Some(reference) = activation_reference(value.get("details"))
        && value["is_error"] == false
        && let Some(body) = bodies.get(reference)
        && value["content"] == serde_json::json!([{"type":"text", "text":body}])
    {
        let receipt = format!(
            "Skill {reference} remains active; its exact instructions are reattached above."
        );
        value["content"] = serde_json::json!([{"type":"text", "text":receipt}]);
        return true;
    }
    match value {
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= project_code_result(value, bodies);
            }
            changed
        }
        Value::Object(fields) => {
            let mut changed = false;
            for value in fields.values_mut() {
                changed |= project_code_result(value, bodies);
            }
            changed
        }
        _ => false,
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
        let projector = ActivatedSkillProjector::new(
            [(active.to_owned(), "full instructions".to_owned())].into(),
        );
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

    #[test]
    fn code_mode_projection_preserves_errors_and_nonmatching_output() {
        let active =
            "skill:review:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let projector = ActivatedSkillProjector::new(
            [(active.to_owned(), "full instructions".to_owned())].into(),
        );
        let Message::Tool { result: loaded } = result("loaded", active) else {
            unreachable!()
        };
        let loaded = serde_json::to_value(loaded).unwrap();
        let mut error = loaded.clone();
        error["is_error"] = json!(true);
        let mut different = loaded.clone();
        different["content"][0]["text"] = json!("Other output that must survive.");
        let mut unpinned = loaded.clone();
        unpinned["details"]["reference"] = json!("skill:unknown:unactivated");
        let output = json!([loaded.clone(), error, different, unpinned, {"nested": loaded}]);
        let projected = projector
            .project(vec![Message::Tool {
                result: ToolResult {
                    call_id: "code".to_owned(),
                    name: "code_mode".to_owned(),
                    content: vec![ContentBlock::text(output.to_string())],
                    details: None,
                    is_error: false,
                },
            }])
            .unwrap();
        let Message::Tool { result } = &projected[0] else {
            unreachable!()
        };
        let ContentBlock::Text { text } = &result.content[0] else {
            unreachable!()
        };
        let actual: serde_json::Value = serde_json::from_str(text).unwrap();
        let receipt = json!([{
            "type": "text",
            "text": format!("Skill {active} remains active; its exact instructions are reattached above.")
        }]);
        assert_eq!(actual[0]["content"], receipt);
        assert_eq!(actual[4]["nested"]["content"], receipt);
        for index in [1, 2, 3] {
            assert_eq!(actual[index], output[index]);
        }
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
