use super::{AutomationMutation, AutomationSpec, LocalHost};
use crate::{capabilities, host::HostConfig};
use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, CommandId, EffectRecovery, SessionId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) fn binding(
    host: Arc<HostConfig>,
    actor: AgentId,
    session: SessionId,
    command: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new("renoa-automation-manage-v7",Arc::new(Manage{host:LocalHost{config:host},actor,session,command,spec:ToolSpec{
        name:capabilities::AUTOMATION_MANAGE.to_owned(),
        description:"Manage Host-owned scheduled tasks. List first for compact automation summaries and current_agent. Use get to read the full standing task before editing. An agent manages its own automations; managing another agent's automations needs the enabled renoa.agents plugin. Create only when the user requests scheduled work. Update the existing automation using its exact revision and full spec; enabled=false pauses future occurrences. To remove an automation, use delete with its id and exact expected_revision. Deletion removes it from automation listings and prevents future scheduling or manual runs; past results remain available through automation_results, and any already-admitted run finishes. Delete only when requested. run_now queues one manual occurrence; for a one-time schedule it also disarms the future run. Explicit run_now can run a disabled task again. One-time schedules use kind=once with at set to an absolute future timestamp including a UTC offset or Z. They disarm atomically when queued, retain their result/history, and catch up once after downtime. To re-arm a consumed task, update it with a new future date and enabled=true. Repeating schedules use kind=cron with a 5-field expression and the IANA timezone its times are in; runs must be at least 5 minutes apart. No overlapping occurrences; downtime coalesces missed times into one run, a cron run more than half the gap to its next run late is skipped instead (automation_results shows why), and each run starts with a line naming the automation, its due time, and how late it started. Each run is sent as a message into the conversation where the agent created the automation for itself, so the result appears there and that conversation remembers it; an automation created for another agent, or outside a conversation, runs in a conversation of its own. Results are also kept in the agent's Host inbox. Files must be written by an available tool to persist artifacts. Do not claim a schedule exists before this tool succeeds.".to_owned(),
        input_schema:input_schema()
    }}),EffectRecovery::SafeToReplay)
}

pub(super) fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "get", "create", "update", "run_now", "delete"],
                "description": "list: optional agent_id and cursor. get or run_now: id. create: spec. update: id, expected_revision, and spec. delete: id and expected_revision. Pass only fields for the selected action."
            },
            "agent_id": {"type": "string", "format": "uuid"},
            "cursor": {"type": "string", "format": "uuid"},
            "id": {"type": "string", "format": "uuid"},
            "expected_revision": {"type": "integer", "minimum": 1},
            "spec": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string", "format": "uuid"},
                    "name": {"type": "string", "minLength": 1, "maxLength": 512},
                    "prompt": {"type": "string", "minLength": 1, "maxLength": 32768},
                    "enabled": {"type": "boolean"},
                    "schedule": {
                        "type": "object",
                        "description": "kind=once runs one time: pass at. kind=cron repeats: pass expression and timezone. Pass only fields for that kind.",
                        "properties": {
                            "kind": {"type": "string", "enum": ["once", "cron"]},
                            "at": {"type": "string", "format": "date-time", "maxLength": 128, "description": "Future timestamp with a UTC offset or Z, e.g. 2026-10-01T09:00:00+05:30"},
                            "expression": {"type": "string", "maxLength": 128, "description": "minute hour day-of-month month weekday. Examples: \"0 9 * * *\" daily at 09:00; \"30 9 * * 1-5\" weekdays at 09:30; \"*/15 * * * *\" every 15 minutes; \"0 */6 * * *\" every 6 hours; \"0 10 1 * *\" monthly on the 1st at 10:00; \"0 18 * * SAT,SUN\" weekends at 18:00"},
                            "timezone": {"type": "string", "description": "IANA timezone the expression's times are in, e.g. Asia/Kolkata or Asia/Seoul"}
                        },
                        "required": ["kind"],
                        "additionalProperties": false
                    }
                },
                "required": ["agent_id", "name", "prompt", "schedule", "enabled"],
                "additionalProperties": false
            }
        },
        "required": ["action"],
        "additionalProperties": false
    })
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
        agent_id: Option<AgentId>,
        cursor: Option<Uuid>,
    },
    Get {
        id: Uuid,
    },
    Create {
        spec: AutomationSpec,
    },
    Update {
        id: Uuid,
        expected_revision: i64,
        spec: AutomationSpec,
    },
    RunNow {
        id: Uuid,
    },
    Delete {
        id: Uuid,
        expected_revision: i64,
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
                return Err(ToolError::cancelled(
                    "automation management cancelled",
                    false,
                ));
            }
            if call.name != capabilities::AUTOMATION_MANAGE {
                return Err(ToolError::invalid_input("wrong automation tool binding"));
            }
            let input: Input = serde_json::from_value(call.arguments)
                .map_err(|e| ToolError::invalid_input(e.to_string()))?;
            let actor = self.actor;
            let result = if let Input::List { agent_id, cursor } = input {
                let records = self
                    .host
                    .list_automations(actor, agent_id.unwrap_or(actor), cursor)
                    .await
                    .map_err(|e| ToolError::invalid_input(e.to_string()))?;
                let cursor = if records.len() == 20 {
                    records.last().map(|r| r.id)
                } else {
                    None
                };
                let summaries:Vec<_>=records.iter().map(|r|json!({"id":r.id,"agent_id":r.spec.agent_id,"revision":r.revision,"name":r.spec.name,"schedule":r.spec.schedule,"enabled":r.spec.enabled,"next_due_ms":r.next_due_ms})).collect();
                json!({"current_agent":actor,"automations":summaries,"next_cursor":cursor})
            } else if let Input::Get { id } = input {
                json!({"automation":self.host.automation(actor,id).await.map_err(|e|ToolError::invalid_input(e.to_string()))?})
            } else {
                let mutation = match input {
                    Input::Create { spec } => AutomationMutation::Create { spec },
                    Input::Update {
                        id,
                        expected_revision,
                        spec,
                    } => AutomationMutation::Update {
                        id,
                        expected_revision,
                        spec,
                    },
                    Input::RunNow { id } => AutomationMutation::RunNow { id },
                    Input::Delete {
                        id,
                        expected_revision,
                    } => AutomationMutation::Delete {
                        id,
                        expected_revision,
                    },
                    Input::List { .. } | Input::Get { .. } => {
                        return Err(ToolError::invalid_input("invalid mutation"));
                    }
                };
                let deleting = matches!(mutation, AutomationMutation::Delete { .. });
                let operation =
                    crate::mcp::oauth_operation_id(self.session, self.command, &call.id);
                // Reuse the Host's stable operation identity, independent of model/surface.
                let operation =
                    crate::stable_id::stable_id(&format!("renoa.automation.manage.v1:{operation}"));
                let now = crate::TurnObservation::now()
                    .map_err(|e| ToolError::invalid_input(e.to_string()))?
                    .unix_milliseconds();
                let record = self
                    .host
                    .manage_automation_from(
                        actor,
                        Some(self.session),
                        operation,
                        mutation,
                        now,
                        cancellation,
                    )
                    .await
                    .map_err(|error| match error {
                        crate::LocalHostError::Automation(super::AutomationError::Cancelled) => {
                            ToolError::cancelled(
                                "automation management cancelled before commit",
                                false,
                            )
                        }
                        error => ToolError::invalid_input(error.to_string()),
                    })?;
                if deleting {
                    json!({"deleted":true,"id":record.id,"revision":record.revision})
                } else {
                    json!({"automation":record,"next_due":jiff::Timestamp::from_millisecond(record.next_due_ms).map_err(|e|ToolError::invalid_input(e.to_string()))?.to_string()})
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

#[cfg(test)]
mod input_tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::Input;

    #[test]
    fn action_and_schedule_variants_still_reject_foreign_or_missing_fields() {
        assert!(
            serde_json::from_value::<Input>(json!({"action": "list", "id": Uuid::nil()})).is_err()
        );
        assert!(serde_json::from_value::<Input>(json!({"action": "create"})).is_err());
        assert!(
            serde_json::from_value::<Input>(json!({
                "action": "create",
                "spec": {
                    "agent_id": Uuid::nil(),
                    "name": "Digest",
                    "prompt": "Summarize",
                    "schedule": {"kind": "cron", "expression": "0 9 * * *"},
                    "enabled": true
                }
            }))
            .is_err()
        );
    }
}
