//! The `agent_manage` capability: canonical creation, listing, and renames.
//!
//! The tool carries no policy of its own. It translates one model call into the
//! canonical creation, listing, or rename operation, and every rule that
//! matters (trusted actors, presets, exact capabilities, idempotent operations)
//! is enforced there.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{AgentCreateRequest, LocalHost, MAX_AGENT_PAGE, RenameAgent};
use crate::host::HostConfig;
use crate::stable_id::stable_id;
use crate::{
    AgentCreationOrigin, AgentCreator, AgentDefinition, AgentPresetId, capabilities, presets,
};

const TOOL_NAME: &str = capabilities::AGENT_MANAGE;
const BINDING_REVISION: &str = "renoa-agent-manage-v1";
const CREATE_OPERATION_DOMAIN: &str = "renoa.agent.create.operation.v1";
const RENAME_OPERATION_DOMAIN: &str = "renoa.agent.rename.operation.v1";

/// Binds the canonical agent-management capability to one live session.
pub(crate) fn binding(
    host: Arc<HostConfig>,
    actor: AgentId,
    session: SessionId,
    command: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(Manage {
            host: LocalHost { config: host },
            actor,
            session,
            command,
            spec: ToolSpec {
                name: TOOL_NAME.to_owned(),
                description: description(),
                input_schema: input_schema(),
            },
        }),
        EffectRecovery::SafeToReplay,
    )
}

struct Manage {
    host: LocalHost,
    actor: AgentId,
    session: SessionId,
    command: Option<CommandId>,
    spec: ToolSpec,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    List {
        cursor: Option<AgentId>,
    },
    Create {
        preset_id: AgentPresetId,
        name: String,
        instructions: Option<String>,
        #[serde(default)]
        tools: BTreeSet<String>,
        #[serde(default)]
        connections: BTreeSet<String>,
    },
    Rename {
        id: AgentId,
        expected_name: String,
        name: String,
    },
}

/// The compact roster entry one page reports.
#[derive(Serialize)]
struct AgentSummary<'a> {
    id: AgentId,
    name: &'a str,
    preset_id: Option<&'a AgentPresetId>,
    tools: &'a BTreeSet<String>,
    connections: &'a BTreeSet<String>,
}

impl<'a> From<&'a AgentDefinition> for AgentSummary<'a> {
    fn from(definition: &'a AgentDefinition) -> Self {
        Self {
            id: definition.id,
            name: &definition.name,
            preset_id: definition.preset_id.as_ref(),
            tools: &definition.tool_selection.tools,
            connections: &definition.connections,
        }
    }
}

impl Tool for Manage {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input("wrong tool binding"));
            }
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            let input: Input = serde_json::from_value(call.arguments)
                .map_err(|error| ToolError::invalid_input(error.to_string()))?;
            let result = match input {
                Input::List { cursor } => {
                    let page = self
                        .host
                        .list_agent_definitions(cursor, MAX_AGENT_PAGE)
                        .await
                        .map_err(tool_error)?;
                    json!({
                        "current_agent":self.actor,
                        "agents":page.agents.iter().map(AgentSummary::from).collect::<Vec<_>>(),
                        "next_cursor":page.next_cursor,
                    })
                }
                Input::Create {
                    preset_id,
                    name,
                    instructions,
                    tools,
                    connections,
                } => {
                    let operation = self.operation(CREATE_OPERATION_DOMAIN, &call.id);
                    let mut request = AgentCreateRequest::new(operation, preset_id, name)
                        .with_tools(tools)
                        .with_connections(connections);
                    if let Some(instructions) = instructions {
                        request = request.with_instructions(instructions);
                    }
                    let definition = self
                        .host
                        .create_agent(
                            AgentCreator::Agent {
                                agent_id: self.actor,
                            },
                            AgentCreationOrigin::AgentTool,
                            request,
                            cancellation,
                        )
                        .await
                        .map_err(tool_error)?;
                    let summary = AgentSummary::from(&definition);
                    json!({
                        "id":summary.id,
                        "name":summary.name,
                        "preset_id":summary.preset_id,
                        "created_by":self.actor,
                        "tools":summary.tools,
                        "connections":summary.connections,
                    })
                }
                Input::Rename {
                    id,
                    expected_name,
                    name,
                } => {
                    let operation = self.operation(RENAME_OPERATION_DOMAIN, &call.id);
                    let definition = self
                        .host
                        .rename_agent(
                            self.actor,
                            operation,
                            RenameAgent {
                                id,
                                expected_name,
                                name,
                            },
                            cancellation,
                        )
                        .await
                        .map_err(tool_error)?;
                    json!({"id":definition.id,"name":definition.name})
                }
            };
            Ok(ToolOutput {
                content: vec![renoa_agent::ContentBlock::text(result.to_string())],
                details: None,
                is_error: false,
            })
        })
    }
}

impl Manage {
    /// Derives one durable operation identity from the model call that asked for
    /// it, so a retried call replays instead of creating a second agent.
    fn operation(&self, domain: &str, call_id: &str) -> Uuid {
        stable_id(&format!(
            "{domain}:{}",
            crate::mcp::oauth_operation_id(self.session, self.command, call_id)
        ))
    }
}

fn cancelled() -> ToolError {
    ToolError::cancelled("agent management cancelled before commit", false)
}

fn tool_error(error: crate::LocalHostError) -> ToolError {
    match error {
        crate::LocalHostError::AgentCancelled => cancelled(),
        error => ToolError::invalid_input(error.to_string()),
    }
}

fn description() -> String {
    let mut description = String::from(
        "Create, list, or rename durable agents on this Host. Create only when the user asks for an agent with its own job or instructions. Presets:\n",
    );
    for preset in presets::catalog() {
        writeln!(description, "- {}: {}", preset.id(), preset.description())
            .expect("writing to a String cannot fail");
    }
    description.push_str(
        "Use the user's chosen name; otherwise choose a short job name of 1-3 words, such as X Desk, News, or Research. Avoid technical slugs, ids, and redundant agent or manager labels. To rename, list first and pass the exact current name as expected_name; identity, sessions, capabilities, and connections stay the same. Select only the capabilities the job needs, and reuse exact connection ids from extension_manage list. Creation persists a separate agent with its own sessions. A repeated identical call reuses the same agent. List returns compact pages; pass next_cursor back as cursor until it is absent. Scheduling is not available in this operation; use routine_manage.",
    );
    description
}

fn input_schema() -> serde_json::Value {
    let preset_ids: Vec<&str> = presets::catalog()
        .map(|preset| preset.id().as_str())
        .collect();
    let selectable = capabilities::selectable_names();
    json!({"type":"object","properties":{
        "action":{"enum":["list","create","rename"]},
        "cursor":{"type":["string","null"],"format":"uuid","description":"For list only: exact next_cursor from the preceding page, or omit for the first page."},
        "preset_id":{"enum":preset_ids,"description":"For create only: the creation preset whose job matches the request."},
        "name":{"type":"string","minLength":1,"maxLength":512},
        "instructions":{"type":"string","minLength":1,"maxLength":32768,"description":"For create only, and required by presets that take caller instructions: the agent's own standing instructions."},
        "tools":{"type":"array","uniqueItems":true,"items":{"enum":selectable},"description":"For create or later capability edits: exact capability names. Omit to accept the preset's own baseline."},
        "connections":{"type":"array","maxItems":64,"uniqueItems":true,"items":{"type":"string","maxLength":256},"description":"For create only: exact existing Host connection ids this agent may use."},
        "id":{"type":"string","format":"uuid"},
        "expected_name":{"type":"string","description":"For rename only: the agent's exact current name."}
    },"required":["action"],"additionalProperties":false,"oneOf":[
        {"properties":{"action":{"const":"list"},"preset_id":false,"name":false,"instructions":false,"tools":false,"connections":false,"id":false,"expected_name":false}},
        {"properties":{"action":{"const":"create"},"cursor":false,"id":false,"expected_name":false},"required":["preset_id","name"]},
        {"properties":{"action":{"const":"rename"},"cursor":false,"preset_id":false,"instructions":false,"tools":false,"connections":false},"required":["id","expected_name","name"]}
    ]})
}

#[cfg(test)]
mod tests {
    use super::input_schema;

    #[test]
    fn creation_schema_presents_the_exact_native_capability_catalog() {
        let schema = input_schema();
        let tools = &schema["properties"]["tools"];
        assert_eq!(
            tools["items"]["enum"],
            serde_json::json!([
                "read_file",
                "edit_file",
                "write_file",
                "bash",
                "grep",
                "find",
                "git_changes",
                "git_diff",
                "git_show",
                "extension_manage",
                "agent_manage",
                "routine_manage",
                "routine_results",
                "tool_search",
                "tool_load",
                "tool_execute",
                "skill_search",
                "skill_load",
                "agent_documents",
            ])
        );
        assert_eq!(
            tools["description"],
            "For create or later capability edits: exact capability names. Omit to accept the preset's own baseline."
        );
    }
}
