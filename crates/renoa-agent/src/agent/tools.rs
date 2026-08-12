use std::sync::Arc;

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink, ContentBlock, Message, Tool, ToolCall, ToolExecutionMode,
    ToolOutput, ToolResult, ToolUpdates,
    events::{append_message, emit_event},
};

use super::{Agent, AgentError};

impl Agent {
    pub(super) async fn reject_length_stopped_calls(
        &mut self,
        calls: &[ToolCall],
        sink: Option<&dyn AgentEventSink>,
    ) {
        for call in calls {
            let result = error_tool_result(
                call,
                "Tool call was not executed because the model response reached its token limit.",
            );
            append_message(sink, &mut self.state.messages, Message::Tool { result }).await;
        }
    }

    pub(super) async fn execute_tool_calls(
        &mut self,
        calls: &[ToolCall],
        cancellation: &CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<(), AgentError> {
        let has_exclusive_tool = calls.iter().any(|call| {
            self.find_tool(&call.name)
                .is_some_and(|tool| tool.execution_mode() == ToolExecutionMode::Sequential)
        });
        if self.config.tool_execution == ToolExecutionMode::Parallel && !has_exclusive_tool {
            self.execute_tools_parallel(calls, cancellation, sink).await
        } else {
            self.execute_tools_sequential(calls, cancellation, sink)
                .await
        }
    }

    async fn execute_tools_sequential(
        &mut self,
        calls: &[ToolCall],
        cancellation: &CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<(), AgentError> {
        for (index, call) in calls.iter().enumerate() {
            emit_event(sink, AgentEvent::ToolExecutionStart { call: call.clone() }).await;
            let (progress_sender, mut progress_receiver) = mpsc::channel(1);
            let execution = execute_tool(
                index,
                call.clone(),
                self.find_tool(&call.name),
                cancellation.child_token(),
                progress_sender,
            );
            tokio::pin!(execution);
            let result = loop {
                tokio::select! {
                    biased;
                    Some(progress) = progress_receiver.recv() => {
                        emit_progress(sink, call, progress.update).await;
                    }
                    result = &mut execution => break result.1,
                }
            };
            drain_progress(sink, calls, &mut progress_receiver).await;
            emit_tool_end(sink, call.clone(), result.clone()).await;
            append_message(sink, &mut self.state.messages, Message::Tool { result }).await;
            if cancellation.is_cancelled() {
                self.reject_unstarted_calls(&calls[index + 1..], sink).await;
                emit_event(sink, AgentEvent::TurnEnd).await;
                return Err(AgentError::Cancelled);
            }
        }
        Ok(())
    }

    async fn execute_tools_parallel(
        &mut self,
        calls: &[ToolCall],
        cancellation: &CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<(), AgentError> {
        let mut started = 0;
        for call in calls {
            emit_event(sink, AgentEvent::ToolExecutionStart { call: call.clone() }).await;
            started += 1;
            if cancellation.is_cancelled() {
                break;
            }
        }

        let (progress_sender, mut progress_receiver) = mpsc::channel(1);
        let mut executions = calls[..started]
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, call)| {
                execute_tool(
                    index,
                    call.clone(),
                    self.find_tool(&call.name),
                    cancellation.child_token(),
                    progress_sender.clone(),
                )
            })
            .collect::<FuturesUnordered<_>>();
        drop(progress_sender);

        let mut results = vec![None; calls.len()];
        while !executions.is_empty() {
            tokio::select! {
                biased;
                Some(progress) = progress_receiver.recv() => {
                    emit_progress(sink, &calls[progress.index], progress.update).await;
                }
                Some((index, result)) = executions.next() => {
                    drain_progress(sink, calls, &mut progress_receiver).await;
                    emit_tool_end(sink, calls[index].clone(), result.clone()).await;
                    results[index] = Some(result);
                }
            }
        }
        drain_progress(sink, calls, &mut progress_receiver).await;

        for (index, call) in calls.iter().enumerate() {
            let result = results[index].take().unwrap_or_else(|| {
                error_tool_result(
                    call,
                    "Tool call was not executed because the run was cancelled.",
                )
            });
            append_message(sink, &mut self.state.messages, Message::Tool { result }).await;
        }
        if cancellation.is_cancelled() {
            emit_event(sink, AgentEvent::TurnEnd).await;
            return Err(AgentError::Cancelled);
        }
        Ok(())
    }

    async fn reject_unstarted_calls(
        &mut self,
        calls: &[ToolCall],
        sink: Option<&dyn AgentEventSink>,
    ) {
        for call in calls {
            let result = error_tool_result(
                call,
                "Tool call was not executed because the run was cancelled.",
            );
            append_message(sink, &mut self.state.messages, Message::Tool { result }).await;
        }
    }

    fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }
}

struct Progress {
    index: usize,
    update: ToolOutput,
}

async fn execute_tool(
    index: usize,
    call: ToolCall,
    tool: Option<Arc<dyn Tool>>,
    cancellation: CancellationToken,
    progress: mpsc::Sender<Progress>,
) -> (usize, ToolResult) {
    let result = if cancellation.is_cancelled() {
        error_tool_result(&call, "Tool execution was cancelled.")
    } else if let Some(tool) = tool {
        let (updates, mut update_receiver) = ToolUpdates::channel();
        let mut execution = tool.execute(call.clone(), cancellation.child_token(), updates.clone());
        let outcome = loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break None,
                Some(update) = update_receiver.recv() => {
                    let _ = progress.send(Progress { index, update }).await;
                }
                outcome = &mut execution => break Some(outcome),
            }
        };
        updates.close();
        update_receiver.close();
        while let Ok(update) = update_receiver.try_recv() {
            let _ = progress.send(Progress { index, update }).await;
        }
        match outcome {
            None => error_tool_result(&call, "Tool execution was cancelled."),
            Some(Ok(output)) => ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: output.content,
                details: output.details,
                is_error: false,
            },
            Some(Err(error)) => error_tool_result(&call, &error.to_string()),
        }
    } else {
        error_tool_result(&call, &format!("Tool `{}` is not available.", call.name))
    };
    (index, result)
}

async fn drain_progress(
    sink: Option<&dyn AgentEventSink>,
    calls: &[ToolCall],
    progress: &mut mpsc::Receiver<Progress>,
) {
    while let Ok(progress) = progress.try_recv() {
        emit_progress(sink, &calls[progress.index], progress.update).await;
    }
}

async fn emit_progress(sink: Option<&dyn AgentEventSink>, call: &ToolCall, update: ToolOutput) {
    emit_event(
        sink,
        AgentEvent::ToolExecutionUpdate {
            call: call.clone(),
            update,
        },
    )
    .await;
}

async fn emit_tool_end(sink: Option<&dyn AgentEventSink>, call: ToolCall, result: ToolResult) {
    emit_event(sink, AgentEvent::ToolExecutionEnd { call, result }).await;
}

fn error_tool_result(call: &ToolCall, message: &str) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(message)],
        details: None,
        is_error: true,
    }
}
