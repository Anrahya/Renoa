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

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    List {
        #[schemars(with = "Option<String>")]
        cursor: Option<AgentId>,
    },
    Create {
        #[schemars(with = "Option<String>")]
        preset_id: Option<AgentPresetId>,
        name: String,
        instructions: Option<String>,
        #[serde(default)]
        tools: Option<BTreeSet<String>>,
        model: Option<crate::AgentModelSelection>,
        behavior: Option<crate::AgentBehavior>,
        documents: Option<crate::AgentDocuments>,
        #[serde(default)]
        connections: BTreeSet<String>,
    },
    Rename {
        #[schemars(with = "String")]
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
                    model,
                    behavior,
                    documents,
                    connections,
                } => {
                    let operation = self.operation(CREATE_OPERATION_DOMAIN, &call.id);
                    let request = AgentCreateRequest {
                        operation_id: operation,
                        preset_id,
                        name,
                        instructions,
                        tools,
                        connections,
                        model,
                        behavior,
                        documents,
                        automation: None,
                    };
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
        "Create, list, or rename durable agents on this Host. Create only when the user asks. Supply name and instructions directly; optionally select a preset for defaults:\n",
    );
    for preset in presets::catalog() {
        writeln!(description, "- {}: {}", preset.id(), preset.description())
            .expect("writing to a String cannot fail");
    }
    description.push_str(
        "Use the user's chosen name; otherwise choose a short job name of 1-2 words, such as X Desk, News, or Research. Avoid technical slugs, ids, and redundant agent or manager labels. To rename, list first and pass the exact current name as expected_name; identity, sessions, capabilities, and connections stay the same. Every agent receives plugin management and discovery. tools selects machine access only; you cannot grant machine tools to yourself. Explicit settings replace template defaults. Select only the machine tools the job needs, and reuse exact connection ids from plugin_manage list. Creation persists a separate agent with its own sessions. A repeated identical call reuses the same agent. List returns compact pages; pass next_cursor back as cursor until it is absent. Scheduling is not available in this operation; use automation_manage.",
    );
    description
}

fn input_schema() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| settings.inline_subschemas = true);
    let mut schema = crate::plugins::model_schema(
        settings
            .into_generator()
            .into_root_schema_for::<Input>()
            .to_value(),
    );
    schema["properties"]["tools"] = json!({"type":"array", "uniqueItems":true, "items":{"enum":capabilities::selectable_names()},
        "description":"For create: replace the template's machine grants exactly. [] grants no machine tools. Every agent still receives plugin management, discovery, and invocation."});
    schema
}
