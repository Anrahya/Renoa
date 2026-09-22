use std::{collections::BTreeMap, num::NonZeroU32, sync::Arc};

use renoa_agent::{ContentBlock, Message, ToolCall, ToolSpec};
use renoa_kernel::{
    AgentId, Command, CommandId, EffectBatchId, EffectId, EffectOutcome, EffectRecovery,
    LoopDecision, LoopInput, LoopPlugin, NewEvent, OperationId, RuntimeManifest, SessionId,
    SettledEffect, SettledEffectBatch,
};
use serde_json::json;

use super::{AgentLoop, LoopTool};
use crate::{
    AgentCommand, AgentLoopConfig, FullHistoryStrategy, MESSAGE_EVENT_KIND,
    format::{LoopPhase, checkpoint},
    pending_tools::PendingToolCalls,
};

#[test]
fn definite_tool_adapter_failure_balances_the_assistant_calls() {
    let first = call("first");
    let second = call("second");
    let binding = "renoa.agent.tool/test";
    let loop_plugin = AgentLoop::new(
        AgentLoopConfig::new(
            "test",
            NonZeroU32::new(2).expect("nonzero"),
            NonZeroU32::new(2).expect("nonzero"),
        ),
        Arc::new(FullHistoryStrategy),
        EffectRecovery::SafeToReplay,
        vec![LoopTool {
            spec: ToolSpec {
                name: "test".to_owned(),
                description: "test".to_owned(),
                input_schema: json!({"type": "object"}),
            },
            effect_binding: binding.to_owned(),
            recovery: EffectRecovery::NeverReplay,
        }],
        None,
    );
    let input = LoopInput {
        agent_id: AgentId::new(),
        session_id: SessionId::new(),
        operation_id: OperationId::new(),
        runtime_manifest: RuntimeManifest {
            loop_binding: "test".to_owned(),
            loop_revision: "test".to_owned(),
            checkpoint_schema_version: 4,
            effect_bindings: BTreeMap::from([(binding.to_owned(), "test".to_owned())]),
            config_digest: "test".to_owned(),
        },
        command: Command::new(
            CommandId::new(),
            serde_json::to_value(AgentCommand::text("test")).expect("command"),
        ),
        events: Vec::new(),
        checkpoint: Some(
            checkpoint(LoopPhase::AwaitingTool {
                model_turns: 1,
                pending: PendingToolCalls::new(vec![first.clone(), second.clone()])
                    .expect("pending calls"),
            })
            .expect("checkpoint"),
        ),
        effect_batch: Some(SettledEffectBatch {
            batch_id: EffectBatchId::new(),
            effects: vec![SettledEffect {
                effect_id: EffectId::new(),
                binding: binding.to_owned(),
                binding_revision: "test".to_owned(),
                request: serde_json::to_value(&first).expect("request"),
                outcome: EffectOutcome::Failure {
                    message: "adapter could not encode its result".to_owned(),
                },
            }],
        }),
    };

    let LoopDecision::Fail { events, reason, .. } =
        loop_plugin.decide(input).expect("definite failure")
    else {
        panic!("expected terminal failure");
    };
    assert_eq!(reason, "adapter could not encode its result");
    let expected = [
        (
            first,
            "Tool execution ended without a model-visible result.",
        ),
        (
            second,
            "Tool call was not run because an earlier tool failed.",
        ),
    ]
    .into_iter()
    .map(|(call, message)| {
        NewEvent::new(
            MESSAGE_EVENT_KIND,
            serde_json::to_value(Message::Tool {
                result: renoa_agent::ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: vec![ContentBlock::text(message)],
                    details: None,
                    is_error: true,
                },
            })
            .expect("message"),
        )
    })
    .collect::<Vec<_>>();
    assert_eq!(events, expected);
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "test".to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    }
}
