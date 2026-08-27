use std::{path::PathBuf, str::FromStr as _, sync::Arc};

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
    registry::{SEARCH_RESULT_LIMIT, SkillReference, rank_skills},
    render,
    store::SkillStore,
};
use crate::ALPHA_PROFILE_ID;

pub(super) const SKILL_LOAD_TOOL: &str = "skill_load";
const SKILL_SEARCH_TOOL: &str = "skill_search";
pub(super) const ACTIVATION_DETAIL_KIND: &str = "renoa.skill.activation.v1";
const REGISTRY_REVISION: &str = "renoa-skill-registry-v1";

pub(crate) fn alpha_skill_bindings(
    store: SkillStore,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> Vec<AgentToolBinding> {
    vec![
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/search"),
            Arc::new(SearchTool::new(store.clone(), workspace.clone())),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/load"),
            Arc::new(LoadTool::new(store, workspace, session_id, command_id)),
            EffectRecovery::SafeToReplay,
        ),
    ]
}

struct SearchTool {
    store: SkillStore,
    workspace: PathBuf,
    spec: ToolSpec,
}

impl SearchTool {
    fn new(store: SkillStore, workspace: PathBuf) -> Self {
        Self {
            store,
            workspace,
            spec: ToolSpec {
                name: SKILL_SEARCH_TOOL.to_owned(),
                description: format!(
                    "Find Agent Skills available to Alpha without loading their instructions. Returns at most {SEARCH_RESULT_LIMIT} compact matches and immutable references. Use query `*` to browse, then call skill_load for one exact match. Local global/project .agents sources are rescanned on each call, so additions need no restart."
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
            let workspace = self.workspace.clone();
            let query = input.query;
            let result = tokio::task::spawn_blocking(move || {
                let sync = store.sync(ALPHA_PROFILE_ID, &workspace)?;
                let ranked = rank_skills(store.summaries(ALPHA_PROFILE_ID, &workspace)?, &query)?;
                let matches = ranked
                    .matches
                    .into_iter()
                    .map(|skill| {
                        Ok(SearchMatch {
                            reference: skill.reference()?.to_string(),
                            scope: skill.scope.as_str(),
                            name: skill.name,
                            description: skill.description,
                        })
                    })
                    .collect::<Result<Vec<_>, SkillError>>()?;
                Ok::<_, SkillError>(SearchOutput {
                    matches,
                    total_matches: ranked.total_matches,
                    available_skills: sync.available,
                    rejected_entries: sync.rejected,
                })
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
    store: SkillStore,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl LoadTool {
    fn new(
        store: SkillStore,
        workspace: PathBuf,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            store,
            workspace,
            session_id,
            command_id,
            spec: ToolSpec {
                name: SKILL_LOAD_TOOL.to_owned(),
                description: "Load and durably activate one exact reference returned by skill_search. The full instructions are available immediately, survive restart and compaction, and stay pinned to this immutable revision for the session. A skill supplies instructions; it does not grant tools or permissions.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "reference": {"type": "string"}
                    },
                    "required": ["reference"],
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
            let reference =
                SkillReference::from_str(&input.reference).map_err(|error| skill_error(&error))?;
            let command_id = self.command_id.ok_or_else(|| {
                ToolError::internal("skill_load has no active Host command identity")
            })?;
            require_active(&cancellation, false)?;
            let store = self.store.clone();
            let workspace = self.workspace.clone();
            let session_id = self.session_id;
            let selected = reference.clone();
            let skill = tokio::task::spawn_blocking(move || {
                store.activate(
                    ALPHA_PROFILE_ID,
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
            let content = render::one(&skill).map_err(|error| skill_error(&error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(content)],
                details: Some(json!({
                    "kind": ACTIVATION_DETAIL_KIND,
                    "reference": reference.to_string(),
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
struct SearchOutput {
    matches: Vec<SearchMatch>,
    total_matches: usize,
    available_skills: usize,
    rejected_entries: usize,
}

#[derive(Serialize)]
struct SearchMatch {
    reference: String,
    scope: &'static str,
    name: String,
    description: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadInput {
    reference: String,
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
