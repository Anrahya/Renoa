use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, EffectRecovery, SessionId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{
    SkillError,
    registry::{SEARCH_RESULT_LIMIT, rank_skills},
    render,
    store::SkillStore,
};
use crate::AgentProfileId;

pub(super) const SKILL_LOAD_TOOL: &str = "skill_load";
const SKILL_SEARCH_TOOL: &str = "skill_search";
pub(super) const ACTIVATION_DETAIL_KIND: &str = "renoa.skill.activation.v1";
const REGISTRY_REVISION: &str = "renoa-skill-registry-v4";

pub(crate) fn profile_skill_bindings(
    profile_id: AgentProfileId,
    store: SkillStore,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> Vec<AgentToolBinding> {
    vec![
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/search"),
            Arc::new(SearchTool::new(
                profile_id.clone(),
                store.clone(),
                workspace.clone(),
            )),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/load"),
            Arc::new(LoadTool::new(
                profile_id, store, workspace, session_id, command_id,
            )),
            EffectRecovery::SafeToReplay,
        ),
    ]
}

struct SearchTool {
    profile_id: AgentProfileId,
    store: SkillStore,
    workspace: PathBuf,
    spec: ToolSpec,
}

impl SearchTool {
    fn new(profile_id: AgentProfileId, store: SkillStore, workspace: PathBuf) -> Self {
        Self {
            profile_id,
            store,
            workspace,
            spec: ToolSpec {
                name: SKILL_SEARCH_TOOL.to_owned(),
                description: format!(
                    "Find Agent Skills available to this agent profile without loading their instructions. Returns at most {SEARCH_RESULT_LIMIT} matches containing only name and description. Use query `*` to browse, then call skill_load with one name. Local global/project .agents sources are rescanned on each call, and installed Agent Plugin skills are hot-loaded. Precedence is project, global, then plugin; different plugins cannot silently compete for one name."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Capability or workflow to find; use * to browse."
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for SearchTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let input: SearchInput = decode(&call, SKILL_SEARCH_TOOL)?;
            require_active(&cancellation, false)?;
            let store = self.store.clone();
            let profile_id = self.profile_id.clone();
            let workspace = self.workspace.clone();
            let query = input.query;
            let result = tokio::task::spawn_blocking(move || {
                store.sync(profile_id.as_str(), &workspace)?;
                let matches =
                    rank_skills(store.summaries(profile_id.as_str(), &workspace)?, &query)?
                        .into_iter()
                        .map(|skill| SearchMatch {
                            name: skill.name,
                            description: skill.description,
                        })
                        .collect::<Vec<_>>();
                Ok::<_, SkillError>(matches)
            })
            .await
            .map_err(|error| background_error(&error))?
            .map_err(|error| skill_error(&error))?;
            require_active(&cancellation, true)?;
            json_output(&result)
        })
    }
}

struct LoadTool {
    profile_id: AgentProfileId,
    store: SkillStore,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl LoadTool {
    fn new(
        profile_id: AgentProfileId,
        store: SkillStore,
        workspace: PathBuf,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            profile_id,
            store,
            workspace,
            session_id,
            command_id,
            spec: ToolSpec {
                name: SKILL_LOAD_TOOL.to_owned(),
                description: "Load and durably activate one skill name returned by skill_search. The Host resolves the current project-over-global selection and pins that exact immutable revision for the session. Full instructions are available immediately and survive restart and compaction. A skill supplies instructions; it does not grant tools or permissions.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for LoadTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let input: LoadInput = decode(&call, SKILL_LOAD_TOOL)?;
            let command_id = self.command_id.ok_or_else(|| {
                ToolError::internal("skill_load has no active Host command identity")
            })?;
            require_active(&cancellation, false)?;
            let store = self.store.clone();
            let profile_id = self.profile_id.clone();
            let workspace = self.workspace.clone();
            let session_id = self.session_id;
            let selected = input.name;
            let skill = tokio::task::spawn_blocking(move || {
                store.activate(
                    profile_id.as_str(),
                    &workspace,
                    session_id,
                    command_id,
                    &selected,
                )
            })
            .await
            .map_err(|error| background_error(&error))?
            .map_err(|error| skill_error(&error))?;
            require_active(&cancellation, true)?;
            let reference = render::reference(&skill).map_err(|error| skill_error(&error))?;
            let content = render::one(&skill).map_err(|error| skill_error(&error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(content)],
                details: Some(json!({
                    "kind": ACTIVATION_DETAIL_KIND,
                    "reference": reference,
                })),
                is_error: false,
            })
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
}

#[derive(Serialize)]
struct SearchMatch {
    name: String,
    description: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadInput {
    name: String,
}

fn decode<T: DeserializeOwned>(call: &ToolCall, expected: &str) -> Result<T, ToolError> {
    if call.name != expected {
        return Err(ToolError::invalid_input(format!(
            "received `{}` request at `{expected}`",
            call.name
        )));
    }
    serde_json::from_value(call.arguments.clone())
        .map_err(|error| ToolError::invalid_input(format!("invalid {expected} arguments: {error}")))
}

fn json_output(value: &impl Serialize) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string(value).map_err(|error| {
        ToolError::internal(format!("skill result could not be encoded: {error}"))
    })?;
    Ok(ToolOutput {
        content: vec![ContentBlock::text(content)],
        details: None,
        is_error: false,
    })
}

fn require_active(
    cancellation: &CancellationToken,
    partial_changes_possible: bool,
) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        Err(ToolError::cancelled(
            "skill operation was cancelled",
            partial_changes_possible,
        ))
    } else {
        Ok(())
    }
}

fn background_error(error: &tokio::task::JoinError) -> ToolError {
    ToolError::internal(format!("skill storage task failed: {error}"))
}

fn skill_error(error: &SkillError) -> ToolError {
    let message = error.to_string();
    match error {
        SkillError::Invalid(_) => ToolError::invalid_input(message),
        SkillError::Conflict(_) => ToolError::conflict(message),
        SkillError::NotFound(_) => ToolError::not_found(message),
        SkillError::Io { .. } => ToolError::io(message, false),
        SkillError::Database(_) | SkillError::HostCatalog(_) => ToolError::unavailable(message),
    }
}

#[cfg(test)]
mod tests {
    use renoa_agent::ToolErrorCode;

    use super::{SkillError, skill_error};

    #[test]
    fn storage_failures_keep_specific_model_visible_categories() {
        let io = SkillError::io("read skill", "SKILL.md", std::io::Error::other("fixture"));
        assert_eq!(skill_error(&io).code(), ToolErrorCode::Io);
        assert_eq!(
            skill_error(&SkillError::Database(rusqlite::Error::InvalidQuery)).code(),
            ToolErrorCode::Unavailable
        );
    }
}
