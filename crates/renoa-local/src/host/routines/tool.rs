use super::{LocalHost, RoutineMutation, RoutineSpec};
use crate::{
    host::HostConfig,
    host_storage::{MANIFEST_FILE, read_manifest},
};
use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) fn binding(
    host: Arc<HostConfig>,
    session: SessionId,
    command: Option<CommandId>,
) -> AgentToolBinding {
    let spec = json!({"type":"object","properties":{
        "agent_id":{"type":"string","format":"uuid"},"name":{"type":"string","minLength":1,"maxLength":512},"prompt":{"type":"string","minLength":1,"maxLength":32768},"enabled":{"type":"boolean"},
        "schedule":{"oneOf":[
            {"type":"object","properties":{"kind":{"const":"once"},"at":{"type":"string","format":"date-time","maxLength":128,"description":"One absolute future date/time with explicit UTC offset or Z, e.g. 2026-09-08T14:00:00+05:30. Resolve relative requests using the current date/time and user's timezone."}},"required":["kind","at"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"daily"},"hour":{"type":"integer","minimum":0,"maximum":23},"minute":{"type":"integer","minimum":0,"maximum":59},"timezone":{"type":"string","description":"Explicit IANA timezone, e.g. Asia/Kolkata"}},"required":["kind","hour","minute","timezone"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"interval"},"hours":{"type":"integer","minimum":1,"maximum":8760}},"required":["kind","hours"],"additionalProperties":false}
        ]}},"required":["agent_id","name","prompt","schedule","enabled"],"additionalProperties":false});
    AgentToolBinding::new("renoa-routine-manage-v2",Arc::new(Manage{host:LocalHost{config:host},session,command,spec:ToolSpec{
        name:"routine_manage".to_owned(),
        description:"Manage Host-owned scheduled tasks for persistent specialists. List first for compact routine summaries and current_agent. Use get to read the full standing task before editing. Arcee can manage any specialist; specialists can manage only their own routines. Create only when the user requests scheduled work. Update the existing routine using its exact revision and full spec; enabled=false pauses future occurrences. run_now queues one manual occurrence; for a one-time schedule it also disarms the future run. Explicit run_now can run a disabled task again. One-time schedules use kind=once with at set to an absolute future timestamp including a UTC offset or Z. They disarm atomically when queued, retain their result/history, and catch up once after downtime. To re-arm a consumed task, update it with a new future date and enabled=true. Daily schedules require an explicit IANA timezone; intervals start from creation/rescheduling and use elapsed hours. No overlapping occurrences; downtime coalesces to one catch-up. Results are durable in the agent's Host inbox; connected surfaces deliver them. Scheduled runs have their own persistent session, separate from interactive chat. Files must be written by an available tool to persist artifacts. Do not claim a schedule exists before this tool succeeds.".to_owned(),
        input_schema:json!({"type":"object","properties":{"action":{"enum":["list","get","create","update","run_now"]},"agent_id":{"type":"string","format":"uuid"},"cursor":{"type":"string","format":"uuid"},"id":{"type":"string","format":"uuid"},"expected_revision":{"type":"integer","minimum":1},"spec":spec},"required":["action"],"additionalProperties":false,"oneOf":[
            {"properties":{"action":{"const":"list"},"id":false,"expected_revision":false,"spec":false}},
            {"properties":{"action":{"const":"create"},"agent_id":false,"cursor":false,"id":false,"expected_revision":false},"required":["spec"]},
            {"properties":{"action":{"const":"update"},"agent_id":false,"cursor":false},"required":["id","expected_revision","spec"]},
            {"properties":{"action":{"enum":["get","run_now"]},"agent_id":false,"cursor":false,"spec":false,"expected_revision":false},"required":["id"]}
        ]})
    }}),EffectRecovery::SafeToReplay)
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
        agent_id: Option<AgentId>,
        cursor: Option<Uuid>,
    },
    Get {
        id: Uuid,
    },
    Create {
        spec: RoutineSpec,
    },
    Update {
        id: Uuid,
        expected_revision: i64,
        spec: RoutineSpec,
    },
    RunNow {
        id: Uuid,
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
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("routine management cancelled", false));
            }
            if call.name != "routine_manage" {
                return Err(ToolError::invalid_input("wrong routine tool binding"));
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
            let result = if let Input::List { agent_id, cursor } = input {
                let records = self
                    .host
                    .list_routines(agent_id.unwrap_or(actor), cursor)
                    .await
                    .map_err(|e| ToolError::invalid_input(e.to_string()))?;
                let cursor = if records.len() == 20 {
                    records.last().map(|r| r.id)
                } else {
                    None
                };
                let summaries:Vec<_>=records.iter().map(|r|json!({"id":r.id,"agent_id":r.spec.agent_id,"revision":r.revision,"name":r.spec.name,"schedule":r.spec.schedule,"enabled":r.spec.enabled,"next_due_ms":r.next_due_ms})).collect();
                json!({"current_agent":actor,"routines":summaries,"next_cursor":cursor})
            } else if let Input::Get { id } = input {
                json!({"routine":self.host.routine(id).await.map_err(|e|ToolError::invalid_input(e.to_string()))?})
            } else {
                let mutation = match input {
                    Input::Create { spec } => RoutineMutation::Create { spec },
                    Input::Update {
                        id,
                        expected_revision,
                        spec,
                    } => RoutineMutation::Update {
                        id,
                        expected_revision,
                        spec,
                    },
                    Input::RunNow { id } => RoutineMutation::RunNow { id },
                    Input::List { .. } | Input::Get { .. } => {
                        return Err(ToolError::invalid_input("invalid mutation"));
                    }
                };
                let operation =
                    crate::mcp::oauth_operation_id(self.session, self.command, &call.id);
                // Reuse the Host's stable operation identity, independent of model/surface.
                let operation =
                    super::store::stable_id(&format!("renoa.routine.manage.v1:{operation}"));
                let now = crate::TurnObservation::now()
                    .map_err(|e| ToolError::invalid_input(e.to_string()))?
                    .unix_milliseconds();
                let record = self
                    .host
                    .manage_routine(actor, operation, mutation, now, cancellation)
                    .await
                    .map_err(|error| match error {
                        crate::LocalHostError::Routine(super::RoutineError::Cancelled) => {
                            ToolError::cancelled(
                                "routine management cancelled before commit",
                                false,
                            )
                        }
                        error => ToolError::invalid_input(error.to_string()),
                    })?;
                json!({"routine":record,"next_due":jiff::Timestamp::from_millisecond(record.next_due_ms).map_err(|e|ToolError::invalid_input(e.to_string()))?.to_string()})
            };
            Ok(ToolOutput {
                content: vec![renoa_agent::ContentBlock::text(result.to_string())],
                details: None,
                is_error: false,
            })
        })
    }
}
