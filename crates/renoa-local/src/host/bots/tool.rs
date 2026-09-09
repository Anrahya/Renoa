use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use super::{BotRecipe, BotRecord, LocalHost};
use crate::{
    host::HostConfig,
    host_storage::{MANIFEST_FILE, read_manifest},
};

pub(crate) fn binding(
    host: Arc<HostConfig>,
    session: SessionId,
    command: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new("renoa-bot-manage-v2", Arc::new(Manage {
        host: LocalHost { config: host }, session, command,
        spec: ToolSpec {
            name: "bot_manage".to_owned(),
            description: "Create, list, or rename persistent specialist agents on this Host. Create only when the user asks for a bot with its own job/instructions. Use the user's chosen name, otherwise choose a short job name of 1–3 words, such as X Desk, News, or Research. Avoid technical slugs, IDs, and redundant bot/agent/manager labels. To rename, list first and pass its exact current name as expected_name; identity, sessions, tools, and connections stay the same. Choose a minimal tool set; reuse exact connection names from extension_manage list. Creation persists a separate profile and identity. Return the bot ID so a surface can open its conversation. A repeated tool call reuses the same bot. List returns compact pages; pass next_cursor as cursor until absent. Scheduling is not available in this operation.".to_owned(),
            input_schema: json!({"type":"object","properties":{
                "action":{"enum":["list","create","rename"]},
                "id":{"type":"string","format":"uuid"},"expected_name":{"type":"string"},"name":{"type":"string","minLength":1,"maxLength":512},
                "cursor":{"type":["string","null"],"format":"uuid","description":"For list only: exact next_cursor from the preceding page, or omit for the first page."},
                "recipe":{"type":"object","properties":{
                    "name":{"type":"string","minLength":1,"maxLength":512},
                    "instructions":{"type":"string","minLength":1,"maxLength":32768},
                    "tools":{"type":"array","uniqueItems":true,"items":{"enum":["read_file","write_file","edit_file","bash","grep","find","git_changes","git_diff","git_show","extension_manage","bot_manage"]}},
                    "connections":{"type":"array","maxItems":64,"uniqueItems":true,"items":{"type":"string"}}
                },"required":["name","instructions","tools","connections"],"additionalProperties":false}
            },"required":["action"],"additionalProperties":false,"oneOf":[
                {"properties":{"action":{"const":"list"},"recipe":false,"id":false,"expected_name":false,"name":false}},
                {"properties":{"action":{"const":"create"},"cursor":false,"id":false,"expected_name":false,"name":false},"required":["recipe"]},
                {"properties":{"action":{"const":"rename"},"cursor":false,"recipe":false},"required":["id","expected_name","name"]}
            ]}),
        }
    }), EffectRecovery::SafeToReplay)
}

struct Manage {
    host: LocalHost,
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
        recipe: BotRecipe,
    },
    Rename {
        id: AgentId,
        expected_name: String,
        name: String,
    },
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
            if call.name != "bot_manage" {
                return Err(ToolError::invalid_input("wrong tool binding"));
            }
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("bot management cancelled", false));
            }
            let input: Input = serde_json::from_value(call.arguments)
                .map_err(|e| ToolError::invalid_input(e.to_string()))?;
            let manifest = read_manifest(
                self.host
                    .config
                    .sessions
                    .join(self.session.to_string())
                    .join(MANIFEST_FILE),
            )
            .await
            .map_err(|e| ToolError::invalid_input(e.to_string()))?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("bot management cancelled", false));
            }
            let result = match input {
                Input::List { cursor } => {
                    let page = self
                        .host
                        .list_bots(cursor)
                        .await
                        .map_err(|e| ToolError::invalid_input(e.to_string()))?;
                    json!({"current_agent":manifest.agent_id,"bots":page.bots,"next_cursor":page.next_cursor})
                }
                Input::Rename {
                    id,
                    expected_name,
                    name,
                } => {
                    let operation =
                        crate::mcp::oauth_operation_id(self.session, self.command, &call.id);
                    let hash = Sha256::digest(format!("renoa.bot.rename.v1:{operation}"));
                    let mut bytes = [0; 16];
                    bytes.copy_from_slice(&hash[..16]);
                    let result = self
                        .host
                        .rename_bot(
                            manifest.agent_id,
                            uuid::Uuid::from_bytes(bytes),
                            super::names::RenameBot {
                                id,
                                expected_name,
                                name,
                            },
                            cancellation,
                        )
                        .await
                        .map_err(|e| match e {
                            crate::LocalHostError::BotRenameCancelled => {
                                ToolError::cancelled("bot rename cancelled before commit", false)
                            }
                            error => ToolError::invalid_input(error.to_string()),
                        })?;
                    json!(result)
                }
                Input::Create { recipe } => {
                    let operation =
                        crate::mcp::oauth_operation_id(self.session, self.command, &call.id);
                    let digest = Sha256::digest(format!("renoa.bot.create.v1:{operation}"));
                    let mut bytes = [0; 16];
                    bytes.copy_from_slice(&digest[..16]);
                    let id = AgentId::from_uuid(uuid::Uuid::from_bytes(bytes));
                    let bot = self
                        .host
                        .ensure_bot_with_cancellation(
                            BotRecord {
                                id,
                                created_by: manifest.agent_id,
                                recipe,
                            },
                            cancellation.clone(),
                        )
                        .await
                        .map_err(|e| match e {
                            crate::LocalHostError::BotCreationCancelled => {
                                ToolError::cancelled("bot creation cancelled before commit", false)
                            }
                            error => ToolError::invalid_input(error.to_string()),
                        })?;
                    json!({"id":bot.id,"name":bot.recipe.name,"created_by":bot.created_by,"tools":bot.recipe.tools,"connections":bot.recipe.connections})
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
