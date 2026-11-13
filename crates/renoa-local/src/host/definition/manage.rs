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
const BINDING_REVISION: &str = "renoa-agent-manage-v4";
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
        /// Job rules for the new agent, layered after `<purpose>` without replacing the preset base.
        instructions: Option<String>,
        /// What the new agent is for, in one or two plain sentences its owner can read.
        purpose: Option<String>,
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
                Input::List { cursor } => self.list_agents(cursor).await?,
                Input::Create {
                    preset_id,
                    name,
                    instructions,
                    purpose,
                    tools,
                    model,
                    behavior,
                    documents,
                    connections,
                } => {
                    self.create_agent(
                        &call.id,
                        CreateInput {
                            preset_id,
                            name,
                            instructions,
                            purpose,
                            tools,
                            model,
                            behavior,
                            documents,
                            connections,
                        },
                        cancellation,
                    )
                    .await?
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
                closing: None,
            })
        })
    }
}

struct CreateInput {
    preset_id: Option<AgentPresetId>,
    name: String,
    instructions: Option<String>,
    purpose: Option<String>,
    tools: Option<BTreeSet<String>>,
    model: Option<crate::AgentModelSelection>,
    behavior: Option<crate::AgentBehavior>,
    documents: Option<crate::AgentDocuments>,
    connections: BTreeSet<String>,
}

impl Manage {
    async fn list_agents(&self, cursor: Option<AgentId>) -> Result<serde_json::Value, ToolError> {
        let page = self
            .host
            .list_agent_definitions(cursor, MAX_AGENT_PAGE)
            .await
            .map_err(tool_error)?;
        Ok(json!({
            "current_agent": self.actor,
            "agents": page.agents.iter().map(AgentSummary::from).collect::<Vec<_>>(),
            "next_cursor": page.next_cursor,
        }))
    }

    async fn create_agent(
        &self,
        call_id: &str,
        input: CreateInput,
        cancellation: CancellationToken,
    ) -> Result<serde_json::Value, ToolError> {
        let preset_id = input.preset_id.unwrap_or_else(|| {
            AgentPresetId::new(presets::APP_PRESET_ID).expect("APP_PRESET_ID is valid")
        });
        let operation = self.operation(CREATE_OPERATION_DOMAIN, call_id);
        let request = AgentCreateRequest {
            operation_id: operation,
            preset_id: Some(preset_id),
            name: input.name,
            instructions: None,
            purpose: input.purpose,
            job: input.instructions,
            tools: input.tools,
            connections: input.connections,
            model: input.model,
            behavior: input.behavior,
            documents: input.documents,
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
        Ok(json!({
            "id": summary.id,
            "name": summary.name,
            "preset_id": summary.preset_id,
            "created_by": self.actor,
            "tools": summary.tools,
            "connections": summary.connections,
        }))
    }

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
        "Create, list, or rename durable agents on this Host. Create only when the user asks. Put the agent's job rules in instructions (layered on the preset base rather than replacing it), give a short name and a short plain purpose the owner will read, and select only the machine tools the job needs. Without preset_id, create uses renoa.app.v7; optionally select another preset:\n",
    );
    for preset in presets::catalog() {
        writeln!(description, "- {}: {}", preset.id(), preset.description())
            .expect("writing to a String cannot fail");
    }
    description.push_str(
        "Use the user's chosen name; otherwise choose a short job name of 1-2 words, such as X Desk, News, or Research. Avoid technical slugs, ids, and redundant agent or manager labels. To rename, list first and pass the exact current name as expected_name; identity, sessions, capabilities, and connections stay the same. Every agent receives plugin management and discovery. tools selects machine access only; you cannot grant machine tools to yourself. Explicit behavior, documents, model, and tools replace template defaults; instructions sets the agent's job block instead of replacing the preset base. Select only the machine tools the job needs, and reuse exact connection ids from plugin_manage list. Creation persists a separate agent with its own sessions. A repeated identical call reuses the same agent. List returns compact pages; pass next_cursor back as cursor until it is absent. Scheduling is not available in this operation; use automation_manage.",
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

#[cfg(test)]
mod tests {
    use super::{Input, description, input_schema};

    #[test]
    fn an_agent_can_give_a_new_agent_its_purpose() {
        let input: Input = serde_json::from_value(serde_json::json!({
            "action": "create",
            "name": "Kcal",
            "instructions": "Count calories.",
            "purpose": "Count my calories from photos."
        }))
        .expect("create with a purpose");
        let Input::Create { purpose, .. } = input else {
            panic!("a create");
        };
        assert_eq!(purpose.as_deref(), Some("Count my calories from photos."));
        assert!(
            input_schema().to_string().contains("\"purpose\""),
            "the tool tells the model it can set one"
        );
    }

    #[test]
    fn tool_contract_documents_app_default_job_layering_and_minimal_tools() {
        let schema = input_schema();
        let instructions_desc = schema["properties"]["instructions"]["description"]
            .as_str()
            .expect("instructions description in schema");
        assert!(
            instructions_desc.contains("Job rules") && instructions_desc.contains("preset base"),
            "instructions schema should describe job layering: {instructions_desc}"
        );
        let desc = description();
        assert!(
            desc.contains(crate::presets::APP_PRESET_ID),
            "description should name default preset: {desc}"
        );
        assert!(
            desc.contains("job rules") && desc.contains("select only the machine tools"),
            "description should explain job layering and selective tools: {desc}"
        );
    }
}
