//! A running turn's events, reported as they happen so attached surfaces see
//! the turn before it ends.
//!
//! Complete events (tool calls and intermediate assistant messages) are
//! recorded durably, and the end-of-turn projection records whatever is still
//! missing. Visible text deltas are ephemeral: they are sent to the session
//! loop as live frames and never stored. Reasoning never leaves the Host.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use renoa_agent::{AgentEvent, AgentEventSink, AssistantContent, BoxFuture};
use renoa_control::TaskId;
use renoa_protocol::{CommandId, ExecutionEventKind, LiveActivity};

use crate::{
    bridge::NodeRuntime,
    live_text::LiveText,
    node_log,
    node_store::{LiveLedger, NodeStoreError},
    projection::project_tool_result,
};

/// One piece of ephemeral live activity, on its way to the coordinator.
#[derive(Debug, Clone)]
pub(crate) struct LiveFrame {
    pub(crate) task_id: TaskId,
    pub(crate) command_id: CommandId,
    pub(crate) activity: LiveActivity,
}

pub(crate) struct LiveEvents {
    runtime: Arc<NodeRuntime>,
    task_id: TaskId,
    command_id: CommandId,
    /// Held only to count a report, never across an await.
    ledger: Mutex<LiveLedger>,
    /// Held only to coalesce a delta, never across an await.
    text: Mutex<LiveText>,
    inner: Arc<dyn AgentEventSink>,
}

impl LiveEvents {
    pub(crate) async fn start(
        runtime: Arc<NodeRuntime>,
        task_id: TaskId,
        command_id: CommandId,
        inner: Arc<dyn AgentEventSink>,
    ) -> Result<Self, NodeStoreError> {
        let ledger = runtime.state.live_ledger(command_id).await?;
        Ok(Self {
            runtime,
            task_id,
            command_id,
            ledger: Mutex::new(ledger),
            text: Mutex::new(LiveText::default()),
            inner,
        })
    }

    /// No receiver is a node with no coordinator session, which drops the
    /// frame: live activity is never kept for later.
    fn send_live(&self, activity: LiveActivity) {
        let _ = self.runtime.live.send(LiveFrame {
            task_id: self.task_id,
            command_id: self.command_id,
            activity,
        });
    }

    /// The durable events one agent event completes.
    ///
    /// A model response's text is recorded live only when the response also
    /// calls a tool: that text is an intermediate step. A final answer has no
    /// tool call and is recorded by the end-of-turn projection, and a
    /// compaction summary never offers tools, so it is never recorded as an
    /// assistant message.
    fn complete(&self, event: &AgentEvent) -> Vec<ExecutionEventKind> {
        match event {
            AgentEvent::ModelRequestEnd { response, .. }
                if response
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantContent::ToolCall { .. })) =>
            {
                response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::Text { text, .. } => {
                            Some(ExecutionEventKind::AssistantMessage { text: text.clone() })
                        }
                        _ => None,
                    })
                    .collect()
            }
            AgentEvent::ToolExecutionStart { call } => vec![ExecutionEventKind::ToolStarted {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            }],
            AgentEvent::ToolExecutionEnd { result, .. } => {
                match project_tool_result(result.clone()) {
                    Ok(finished) => vec![finished],
                    Err(error) => {
                        self.warn(&error.to_string());
                        Vec::new()
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    async fn record(&self, kind: ExecutionEventKind) {
        // The ledger only decides what to record live. The end-of-turn
        // projection reconciles against the database, so counts left by a
        // panic cannot lose an event.
        let admitted = self
            .ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(&kind);
        match admitted {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return self.warn(&error.to_string()),
        }
        match self
            .runtime
            .state
            .append_progress(self.command_id, kind)
            .await
        {
            Ok(true) => self.runtime.signal_commit(),
            Ok(false) => {}
            Err(error) => self.warn(&error.to_string()),
        }
    }

    fn warn(&self, error: &str) {
        node_log::event(
            "warn",
            "progress_unrecorded",
            &serde_json::json!({ "command_id": self.command_id, "error": error }),
        );
    }
}

impl AgentEventSink for LiveEvents {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // Live text goes first, so it precedes the durable record that
            // supersedes it on the way to the coordinator.
            let activity = self
                .text
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .observe(&event, Instant::now());
            if let Some(activity) = activity {
                self.send_live(activity);
            }
            for kind in self.complete(&event) {
                self.record(kind).await;
            }
            self.inner.emit(event).await;
        })
    }
}
