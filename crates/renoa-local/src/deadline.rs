use std::{sync::Arc, time::Duration};

use renoa_agent::{
    BoxFuture, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec, ToolUpdates,
};
use tokio_util::sync::CancellationToken;

pub(crate) const DEFAULT_TOOL_DEADLINE: Duration = Duration::from_mins(2);

/// Enforces one total deadline without abandoning the wrapped tool's cleanup.
pub(crate) struct DeadlineTool {
    inner: Arc<dyn Tool>,
    deadline: Duration,
    partial_changes_possible: bool,
}

impl DeadlineTool {
    pub(crate) fn new(
        inner: Arc<dyn Tool>,
        deadline: Duration,
        partial_changes_possible: bool,
    ) -> Self {
        Self {
            inner,
            deadline,
            partial_changes_possible,
        }
    }
}

impl Tool for DeadlineTool {
    fn spec(&self) -> &ToolSpec {
        self.inner.spec()
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        self.inner.execution_mode()
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        let tool = Arc::clone(&self.inner);
        let deadline = self.deadline;
        let partial_changes_possible = self.partial_changes_possible;
        Box::pin(async move {
            let tool_cancellation = cancellation.child_token();
            let execution = tool.execute(call, tool_cancellation.clone(), updates);
            tokio::pin!(execution);
            tokio::select! {
                biased;
                result = &mut execution => result,
                () = cancellation.cancelled() => {
                    tool_cancellation.cancel();
                    match execution.await {
                        Ok(output) => Ok(output),
                        Err(error) if error.outcome_is_unknown() => Err(error),
                        Err(_) => Err(ToolError::cancelled(
                            "tool execution was cancelled after its work stopped",
                            partial_changes_possible,
                        )),
                    }
                }
                () = tokio::time::sleep(deadline) => {
                    tool_cancellation.cancel();
                    match execution.await {
                        Ok(output) => Ok(output),
                        Err(error) if error.outcome_is_unknown() => Err(error),
                        Err(_) => Err(ToolError::timeout(
                            format!("tool execution exceeded its {} deadline", deadline_label(deadline)),
                            partial_changes_possible,
                        )),
                    }
                }
            }
        })
    }
}

fn deadline_label(deadline: Duration) -> String {
    if deadline.subsec_nanos() == 0 {
        format!("{}-second", deadline.as_secs())
    } else {
        format!("{}-millisecond", deadline.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use renoa_agent::{ContentBlock, ToolErrorCode, invoke_tool};
    use serde_json::json;

    use super::*;

    struct BlockingTool {
        stopped: Arc<AtomicBool>,
        spec: ToolSpec,
    }

    impl Tool for BlockingTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        fn execute(
            &self,
            _call: ToolCall,
            cancellation: CancellationToken,
            _updates: ToolUpdates,
        ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
            let stopped = Arc::clone(&self.stopped);
            Box::pin(async move {
                cancellation.cancelled().await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                stopped.store(true, Ordering::SeqCst);
                Err(ToolError::cancelled("stopped", false))
            })
        }
    }

    #[tokio::test]
    async fn deadline_waits_for_tool_cleanup_before_returning_timeout() {
        let stopped = Arc::new(AtomicBool::new(false));
        let tool: Arc<dyn Tool> = Arc::new(BlockingTool {
            stopped: Arc::clone(&stopped),
            spec: ToolSpec {
                name: "blocking".to_owned(),
                description: "blocks".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
        });
        let deadline = DeadlineTool::new(tool, Duration::from_millis(1), false);
        let result = invoke_tool(
            Some(&deadline),
            ToolCall {
                id: "call-1".to_owned(),
                name: "blocking".to_owned(),
                arguments: json!({}),
                thought_signature: None,
                namespace: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("deadline is a definite result");

        assert!(stopped.load(Ordering::SeqCst));
        assert!(result.is_error);
        assert_eq!(
            result.content,
            vec![ContentBlock::text(
                "tool execution exceeded its 1-millisecond deadline"
            )]
        );
        let details = result.details.expect("typed details");
        assert_eq!(details["error"]["code"], json!(ToolErrorCode::Timeout));
        assert_eq!(details["error"]["partial_changes_possible"], json!(false));
    }
}
