use super::LocalHost;
use crate::{
    host::HostConfig,
    host_storage::{MANIFEST_FILE, read_manifest},
};
use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery, SessionId};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) fn binding(host: Arc<HostConfig>, session: SessionId) -> AgentToolBinding {
    AgentToolBinding::new("renoa-routine-results-v1", Arc::new(Results {
        host:LocalHost {config:host}, session,
        spec:ToolSpec {
            name:"routine_results".to_owned(),
            description:"Read results from this Host's scheduled or manually triggered routine runs, even when they ran in another session or surface. Use this when discussing an automation's output; never rerun a task merely to read its result. List returns compact completed-run metadata newest first; pass next_before to page older results. Read with a run ID returns its exact task, output and execution session. Specialists can read only their own results; Arcee must select a specialist agent_id from bot_manage list when listing results. If only an excerpt was provided in chat context, read the run for the full output.".to_owned(),
            input_schema:json!({"type":"object","properties":{"action":{"enum":["list","read"]},"agent_id":{"type":"string","format":"uuid"},"before":{"type":"integer","minimum":1},"id":{"type":"string","format":"uuid"}},"required":["action"],"additionalProperties":false,"oneOf":[{"properties":{"action":{"const":"list"},"id":false}},{"properties":{"action":{"const":"read"},"agent_id":false,"before":false},"required":["id"]}]}),
        },
    }), EffectRecovery::SafeToReplay)
}
struct Results {
    host: LocalHost,
    session: SessionId,
    spec: ToolSpec,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    List {
        agent_id: Option<AgentId>,
        before: Option<i64>,
    },
    Read {
        id: Uuid,
    },
}
impl Tool for Results {
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
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("result lookup cancelled", false));
            }
            if call.name != "routine_results" {
                return Err(ToolError::invalid_input("wrong result tool binding"));
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
            let actor = manifest.agent_id;
            let result = match input {
                Input::List { agent_id, before } => {
                    let runs = self
                        .host
                        .routine_results(actor, agent_id.unwrap_or(actor), before)
                        .await
                        .map_err(|e| ToolError::invalid_input(e.to_string()))?;
                    let next = if runs.len() == 20 {
                        runs.last().map(|r| r.sequence)
                    } else {
                        None
                    };
                    json!({"runs":runs,"next_before":next})
                }
                Input::Read { id } => {
                    json!({"run":self.host.routine_result(actor,id).await.map_err(|e|ToolError::invalid_input(e.to_string()))?})
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
