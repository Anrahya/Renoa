//! The tools a note pass may call. The pass offers every tool a normal turn
//! offers, under the same definitions, so the prompt prefix stays cached; the
//! Host refuses every call but the four the pass needs, and `memory_note`
//! called through the plugin executor rather than directly; each refusal
//! says what to do instead.

use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::{NOTE, RECALL};
use crate::capabilities::{AGENT_DOCUMENTS, AUTOMATION_MANAGE, TOOL_EXECUTE};

const ONLY: &str = "This is a memory pass: only memory_note, memory_recall, agent_documents for USER.md, and automation_manage list or get run here.";

/// `bindings` under the pass's rule, each with its own definition and recovery
/// class.
pub(crate) fn guard(bindings: &[AgentToolBinding]) -> Vec<AgentToolBinding> {
    bindings
        .iter()
        .map(|binding| {
            AgentToolBinding::new(
                format!("memory-pass/{}", binding.revision()),
                Arc::new(Guarded {
                    tool: binding.tool(),
                }),
                binding.recovery(),
            )
        })
        .collect()
}

struct Guarded {
    tool: Arc<dyn Tool>,
}

impl Tool for Guarded {
    fn spec(&self) -> &ToolSpec {
        self.tool.spec()
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        let refusal = refusal(&call.name, &call.arguments);
        Box::pin(async move {
            match refusal {
                Some(refusal) => Err(ToolError::permission_denied(refusal)),
                None => self.tool.execute(call, cancellation, updates).await,
            }
        })
    }
}

/// Why the pass refuses a call of `name` with `arguments`; `None` when it runs.
fn refusal(name: &str, arguments: &Value) -> Option<String> {
    match name {
        NOTE | RECALL => None,
        AGENT_DOCUMENTS => (arguments.get("document").and_then(Value::as_str) != Some("user"))
            .then(|| {
                format!("{ONLY} Edit only USER.md, with document \"user\"; note anything else with memory_note.")
            }),
        AUTOMATION_MANAGE => (!matches!(
            arguments.get("action").and_then(Value::as_str),
            Some("list" | "get")
        ))
        .then(|| {
            format!("{ONLY} You cannot create or change automations here: note an unscheduled promise with memory_note and `[at YYYY-MM-DDTHH:MM]` instead.")
        }),
        TOOL_EXECUTE => {
            let reference = arguments.get("reference").and_then(Value::as_str);
            let inner = reference.and_then(|reference| reference.rsplit(':').next());
            let empty = Value::Null;
            let inner_arguments = arguments.get("arguments").unwrap_or(&empty);
            match inner {
                // Code Mode reaches plugins only through the executor, and its
                // result keeps no plugin result's details, so a note written
                // there would be missing from the pass's receipt.
                Some(NOTE) => Some(format!(
                    "{ONLY} Call memory_note directly, not through code_mode or tool_execute, so the note is in the pass's receipt."
                )),
                Some(inner) if reference.is_some_and(|reference| reference.starts_with("host:")) => {
                    refusal(inner, inner_arguments)
                }
                _ => Some(format!(
                    "{ONLY} Plugin tools other than these do not run during the pass."
                )),
            }
        }
        _ => Some(format!(
            "{ONLY} Call memory_note for anything worth keeping, or reply NONE."
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::refusal;

    #[test]
    fn the_pass_runs_only_its_four_tools_and_says_what_to_do_instead() {
        for (name, arguments) in [
            ("memory_note", json!({"text": "Likes tea"})),
            ("memory_recall", json!({"pattern": "tea"})),
            ("agent_documents", json!({"document": "user"})),
            ("automation_manage", json!({"action": "list"})),
            ("automation_manage", json!({"action": "get", "id": "x"})),
            (
                "tool_execute",
                json!({"reference": "host:renoa.automations:v9:automation_manage", "arguments": {"action": "get"}}),
            ),
        ] {
            assert_eq!(refusal(name, &arguments), None, "{name} {arguments}");
        }
        for (name, arguments, advice) in [
            (
                "agent_documents",
                json!({"document": "soul"}),
                "document \"user\"",
            ),
            (
                "automation_manage",
                json!({"action": "create"}),
                "[at YYYY-MM-DDTHH:MM]",
            ),
            ("memory_zoom", json!({"block": "0-1"}), "Call memory_note"),
            ("bash", json!({"command": "ls"}), "Call memory_note"),
            ("plugin_search", json!({"query": "x"}), "Call memory_note"),
            (
                "tool_execute",
                json!({"reference": "host:renoa.automations:v9:automation_manage", "arguments": {"action": "delete"}}),
                "cannot create or change",
            ),
            (
                "tool_execute",
                json!({"reference": "mcp:github:create_issue", "arguments": {}}),
                "do not run during the pass",
            ),
            (
                "tool_execute",
                json!({"reference": "host:renoa.memory:v3:memory_note", "arguments": {"text": "Likes tea"}}),
                "Call memory_note directly",
            ),
        ] {
            let refused = refusal(name, &arguments).unwrap_or_else(|| panic!("{name} runs"));
            assert!(refused.starts_with("This is a memory pass"), "{refused}");
            assert!(refused.contains(advice), "{name}: {refused}");
        }
    }
}
