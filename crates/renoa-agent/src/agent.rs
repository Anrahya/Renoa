use std::{collections::HashSet, sync::Arc};

use thiserror::Error;

use crate::{
    AgentEvent, AgentEventSink, AgentHandle, AgentState, ContentBlock, ContextProjectionError,
    ContextProjector, Message, Model, ModelError, QueueMode, StopReason, TokenUsage, Tool,
    ToolCallBatchError, ToolExecutionMode, ToolOutcomeUnknown, control::AgentControl,
    events::emit_event,
};

mod run;
mod tools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunResult {
    /// Text blocks from the last assistant response, concatenated in source order.
    pub output: String,
    /// Number of provider invocations made by this run.
    pub model_turns: u32,
    /// Why the final provider response ended.
    pub stop_reason: StopReason,
    /// Sum of all model turns, or `None` if any turn omitted usage.
    pub usage: Option<TokenUsage>,
}

/// Per-run safety limits and queue draining behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentConfig {
    /// Maximum provider invocations allowed by one `prompt()` or `resume()`.
    pub max_model_turns: u32,
    /// Maximum tool calls accepted from one assistant response.
    pub max_tool_calls_per_turn: usize,
    /// Combined capacity of the steering and follow-up queues.
    pub max_queued_messages: usize,
    /// Number of steering messages claimed at each turn boundary.
    pub steering_mode: QueueMode,
    /// Number of follow-up messages claimed when the Agent would stop.
    pub follow_up_mode: QueueMode,
    /// Batch-level tool scheduling. Individual tools may still require serial execution.
    pub tool_execution: ToolExecutionMode,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_model_turns: 32,
            max_tool_calls_per_turn: 64,
            max_queued_messages: 64,
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            tool_execution: ToolExecutionMode::Sequential,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentConfigError {
    #[error("tool name `{0}` is configured more than once")]
    DuplicateToolName(String),
    #[error("queue limit {limit} is below the {pending} messages already accepted")]
    QueueLimitBelowPending { pending: usize, limit: usize },
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("run was cancelled")]
    Cancelled,
    #[error("there is no conversation to resume")]
    NothingToResume,
    #[error("an assistant response needs new user input before it can be resumed")]
    AssistantTail,
    #[error("model invocation failed: {0}")]
    Model(#[from] ModelError),
    #[error("context projection failed: {0}")]
    ContextProjection(#[from] ContextProjectionError),
    #[error("model stream ended without a completed response")]
    IncompleteModelStream,
    #[error("model returned {actual} tool calls; the per-turn limit is {limit}")]
    ToolCallLimit {
        actual: usize,
        limit: usize,
        usage: Option<TokenUsage>,
    },
    #[error("{source}")]
    InvalidToolCallBatch {
        #[source]
        source: ToolCallBatchError,
        usage: Option<TokenUsage>,
    },
    #[error("one or more tool outcomes are unknown")]
    ToolOutcomesUnknown { outcomes: Vec<ToolOutcomeUnknown> },
    #[error("the model exceeded the configured turn limit of {0}")]
    TurnLimit(u32),
}

/// One stateful, provider-neutral agent conversation.
pub struct Agent {
    model: Arc<dyn Model>,
    system_prompt: String,
    state: AgentState,
    control: AgentControl,
    event_sink: Option<Arc<dyn AgentEventSink>>,
    tools: Vec<Arc<dyn Tool>>,
    context_projector: Option<Arc<dyn ContextProjector>>,
    config: AgentConfig,
}

impl Agent {
    #[must_use]
    pub fn new(model: Arc<dyn Model>, system_prompt: impl Into<String>) -> Self {
        let config = AgentConfig::default();
        Self {
            model,
            system_prompt: system_prompt.into(),
            state: AgentState::default(),
            control: AgentControl::new(config.max_queued_messages),
            event_sink: None,
            tools: Vec::new(),
            context_projector: None,
            config,
        }
    }

    /// Restores host-persisted conversation state.
    #[must_use]
    pub fn from_state(
        model: Arc<dyn Model>,
        system_prompt: impl Into<String>,
        state: AgentState,
    ) -> Self {
        let mut agent = Self::new(model, system_prompt);
        agent.state = state;
        agent
    }

    #[must_use]
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Sends lifecycle events to `sink` while prompts run.
    #[must_use]
    pub fn with_event_sink(mut self, sink: Arc<dyn AgentEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Projects transcript context independently for each model request.
    #[must_use]
    pub fn with_context_projector(mut self, projector: Arc<dyn ContextProjector>) -> Self {
        self.context_projector = Some(projector);
        self
    }

    /// Installs the host-selected tools after rejecting ambiguous names.
    ///
    /// # Errors
    ///
    /// Returns [`AgentConfigError::DuplicateToolName`] when two tools advertise
    /// the same name.
    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Result<Self, AgentConfigError> {
        validate_tools(&tools)?;
        self.tools = tools;
        Ok(self)
    }

    /// Replaces the provider adapter for subsequent runs.
    pub fn set_model(&mut self, model: Arc<dyn Model>) {
        self.model = model;
    }

    /// Replaces host instructions for subsequent model requests.
    pub fn set_system_prompt(&mut self, system_prompt: impl Into<String>) {
        self.system_prompt = system_prompt.into();
    }

    /// Atomically replaces advertised tools after validating their names.
    ///
    /// # Errors
    ///
    /// Returns [`AgentConfigError::DuplicateToolName`] without changing the
    /// current tools when the replacement is ambiguous.
    pub fn set_tools(&mut self, tools: Vec<Arc<dyn Tool>>) -> Result<(), AgentConfigError> {
        validate_tools(&tools)?;
        self.tools = tools;
        Ok(())
    }

    /// Replaces run limits and queue modes without discarding accepted input.
    ///
    /// # Errors
    ///
    /// Returns [`AgentConfigError::QueueLimitBelowPending`] when the new queue
    /// limit is smaller than the number of messages already accepted.
    pub fn set_config(&mut self, config: AgentConfig) -> Result<(), AgentConfigError> {
        if let Err(pending) = self.control.set_queue_limit(config.max_queued_messages) {
            return Err(AgentConfigError::QueueLimitBelowPending {
                pending,
                limit: config.max_queued_messages,
            });
        }
        self.config = config;
        Ok(())
    }

    /// Returns a clonable controller for the current and future prompts.
    #[must_use]
    pub fn handle(&self) -> AgentHandle {
        self.control.handle()
    }

    /// Clears conversation state, unresolved tool outcomes, and queued input
    /// without changing this Agent's model, instructions, tools, event sink,
    /// or limits.
    pub fn reset(&mut self) {
        self.state = AgentState::default();
        self.control.clear_queues();
    }

    /// Runs one text prompt to completion.
    ///
    /// Drive the returned future to completion. To stop an active run, call
    /// [`AgentHandle::abort`] and continue polling the future so lifecycle
    /// events and conversation state settle coherently.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unresolved tool outcomes, cancellation, model
    /// or stream failure, or a configured safety limit.
    pub async fn prompt(&mut self, text: impl Into<String>) -> Result<AgentRunResult, AgentError> {
        self.prompt_content(vec![ContentBlock::text(text)]).await
    }

    /// Runs one ordered text/image user message to completion.
    ///
    /// # Errors
    ///
    /// Returns the same typed failures as [`Self::prompt`].
    pub async fn prompt_content(
        &mut self,
        content: Vec<ContentBlock>,
    ) -> Result<AgentRunResult, AgentError> {
        self.ensure_tool_outcomes_known()?;
        let run = self.control.start();
        let sink = self.event_sink.clone();
        emit_event(sink.as_deref(), AgentEvent::AgentStart).await;
        let result = self
            .run(
                Some(Message::User { content }),
                Vec::new(),
                run.cancellation(),
                sink.as_deref(),
            )
            .await;
        emit_event(sink.as_deref(), AgentEvent::AgentEnd).await;
        result
    }

    /// Continues from an existing user or tool-result tail without adding a
    /// second user message.
    ///
    /// A completed assistant tail can be resumed only when steering or
    /// follow-up input is queued. That input is claimed before lifecycle
    /// listeners run, so concurrent queue clearing cannot invalidate the
    /// provider request.
    ///
    /// As with [`Self::prompt`], drive the returned future to completion and
    /// use [`AgentHandle::abort`] for cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::ToolOutcomesUnknown`] before sampling when the
    /// portable state is blocked, [`AgentError::NothingToResume`] for an empty
    /// conversation, and [`AgentError::AssistantTail`] when new user input is
    /// required. Other errors match [`Self::prompt`].
    pub async fn resume(&mut self) -> Result<AgentRunResult, AgentError> {
        self.ensure_tool_outcomes_known()?;
        if self.config.max_model_turns == 0 {
            return Err(AgentError::TurnLimit(0));
        }
        let initial_input = match self.state.messages.last() {
            None => return Err(AgentError::NothingToResume),
            Some(Message::Assistant { .. }) => {
                let mut input = self.control.take_steering(self.config.steering_mode);
                if input.is_empty() {
                    input = self.control.take_follow_up(self.config.follow_up_mode);
                }
                if input.is_empty() {
                    return Err(AgentError::AssistantTail);
                }
                input
            }
            Some(Message::User { .. } | Message::Tool { .. }) => Vec::new(),
        };

        let run = self.control.start();
        let sink = self.event_sink.clone();
        emit_event(sink.as_deref(), AgentEvent::AgentStart).await;
        let result = self
            .run(None, initial_input, run.cancellation(), sink.as_deref())
            .await;
        emit_event(sink.as_deref(), AgentEvent::AgentEnd).await;
        result
    }

    fn ensure_tool_outcomes_known(&self) -> Result<(), AgentError> {
        if self.state.unresolved_tool_outcomes.is_empty() {
            return Ok(());
        }
        Err(AgentError::ToolOutcomesUnknown {
            outcomes: self.state.unresolved_tool_outcomes.clone(),
        })
    }

    pub(super) fn block_on_tool_outcomes(
        &mut self,
        outcomes: Vec<ToolOutcomeUnknown>,
    ) -> AgentError {
        self.state.unresolved_tool_outcomes.clone_from(&outcomes);
        AgentError::ToolOutcomesUnknown { outcomes }
    }
}

fn validate_tools(tools: &[Arc<dyn Tool>]) -> Result<(), AgentConfigError> {
    let mut names = HashSet::with_capacity(tools.len());
    for tool in tools {
        let name = tool.spec().name.as_str();
        if !names.insert(name) {
            return Err(AgentConfigError::DuplicateToolName(name.to_owned()));
        }
    }
    Ok(())
}
