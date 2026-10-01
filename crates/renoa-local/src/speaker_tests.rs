use std::{num::NonZeroU32, sync::Arc};

use futures_util::stream;
use renoa_agent::{
    BoxFuture, Model, ModelEventStream, ModelRequest, Tool, ToolCall, ToolError, ToolOutput,
    ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentLoopConfig, AgentToolBinding, CodeModeBinding, ContextBinding, ModelBinding,
    build_runtime_with_code_mode,
};
use renoa_kernel::{
    EffectAdapter, EffectCompletion, EffectFuture, EffectInvocation, EffectRecovery,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::ToolAccess;

#[test]
fn a_refused_code_mode_executor_still_builds_a_code_mode_runtime() {
    let executor = AgentToolBinding::new(
        "executor-v1",
        Arc::new(Executor),
        EffectRecovery::NeverReplay,
    );
    let [refused] = <[_; 1]>::try_from(ToolAccess::Refused.apply(vec![executor]))
        .unwrap_or_else(|_| panic!("one binding"));
    assert_eq!(refused.revision(), "guest-refused/executor-v1");
    assert_eq!(refused.recovery(), EffectRecovery::NeverReplay);
    let one = NonZeroU32::new(1).expect("nonzero");
    build_runtime_with_code_mode(
        AgentLoopConfig::new("Answer.", one, one),
        ContextBinding::full_history(),
        ModelBinding::new("unused", Arc::new(Unused), EffectRecovery::SafeToReplay),
        Vec::new(),
        CodeModeBinding::new("unused-evaluator", Arc::new(Unused), refused),
    )
    .expect("a guest's Code Mode runtime builds around the refused executor");
}

struct Executor;

impl Tool for Executor {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "tool_execute".to_owned(),
            description: "Runs a plugin tool.".to_owned(),
            input_schema: json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        unreachable!("building a runtime runs no tool")
    }
}

/// A model and evaluator that building a runtime never calls.
struct Unused;

impl Model for Unused {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        Box::pin(stream::empty())
    }
}

impl EffectAdapter for Unused {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async { EffectCompletion::OutcomeUnknown })
    }
}
