use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_kernel::CommandId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    LocalHistoryEntry, LocalHostError, LocalSession, LocalTurnOutcome, LocalWorkspace,
    PiModelOption, PiReasoningLevel,
    alpha_trace::finish_trace,
    host::{HostConfig, initial_reasoning, require_model, resolve_runtime, selected_model},
    selection::{RuntimeSelection, append_selection},
    trace::{ObservedEventSink, TraceRun, TraceStore},
};

/// One durable Alpha Agent instance assembled by the local Host.
pub struct AlphaSession {
    id: Uuid,
    host: Arc<HostConfig>,
    kernel: LocalSession,
    workspace: PathBuf,
    selection_path: PathBuf,
    trace: TraceStore,
    models: Vec<PiModelOption>,
    state: Mutex<SessionState>,
    idle: tokio::sync::Notify,
}

/// Durable resources belonging to one assembled Alpha session.
pub(crate) struct AlphaSessionStorage {
    pub(crate) kernel: LocalSession,
    pub(crate) workspace: PathBuf,
    pub(crate) selection_path: PathBuf,
    pub(crate) trace: TraceStore,
}

struct SessionState {
    model: String,
    reasoning: PiReasoningLevel,
    accepting_work: bool,
    activity: Activity,
}

struct TracedTurn<'a> {
    command_id: CommandId,
    content: Vec<ContentBlock>,
    cancellation: CancellationToken,
    model_id: &'a str,
    reasoning: PiReasoningLevel,
    events: Arc<dyn AgentEventSink>,
    trace: &'a TraceRun,
}

enum Activity {
    Idle,
    Prompt {
        request_id: Uuid,
        cancellation: CancellationToken,
    },
    Configuring,
}

/// Owned model and reasoning choices for surface presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlphaSessionConfiguration {
    pub models: Vec<PiModelOption>,
    pub model: String,
    pub reasoning: PiReasoningLevel,
}

impl AlphaSession {
    pub(crate) fn new(
        id: Uuid,
        host: Arc<HostConfig>,
        storage: AlphaSessionStorage,
        models: Vec<PiModelOption>,
        selection: RuntimeSelection,
    ) -> Self {
        Self {
            id,
            host,
            kernel: storage.kernel,
            workspace: storage.workspace,
            selection_path: storage.selection_path,
            trace: storage.trace,
            models,
            state: Mutex::new(SessionState {
                model: selection.model,
                reasoning: selection.reasoning,
                accepting_work: true,
                activity: Activity::Idle,
            }),
            idle: tokio::sync::Notify::new(),
        }
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Returns the complete kernel-backed transcript for a loading surface.
    ///
    /// # Errors
    ///
    /// Returns durable history corruption or storage failures.
    pub fn history(&self) -> Result<Vec<LocalHistoryEntry>, LocalHostError> {
        Ok(self.kernel.history()?)
    }

    /// Returns an owned snapshot of selectable model configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if a prior panic poisoned session coordination state.
    pub fn configuration(&self) -> Result<AlphaSessionConfiguration, LocalHostError> {
        let state = self.state()?;
        Ok(AlphaSessionConfiguration {
            models: self.models.clone(),
            model: state.model.clone(),
            reasoning: state.reasoning,
        })
    }

    /// Runs one caller-identified turn through fresh Alpha composition.
    ///
    /// Workspace instructions are read for every newly admitted operation.
    /// The resolved behavior then freezes in that operation's kernel manifest.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_turn(
        &self,
        request_id: Uuid,
        content: Vec<ContentBlock>,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let (guard, cancellation, model_id, reasoning) = self.begin_prompt(request_id)?;
        let command_id = CommandId::from_uuid(request_id);
        let trace = self
            .trace
            .start_run(
                command_id,
                &content,
                &self.host.provider,
                &model_id,
                reasoning.as_str(),
            )
            .await?;
        let observed: Arc<dyn AgentEventSink> =
            Arc::new(ObservedEventSink::new(Arc::clone(&trace), events));
        let result = self
            .execute_traced_turn(TracedTurn {
                command_id,
                content,
                cancellation,
                model_id: &model_id,
                reasoning,
                events: observed,
                trace: &trace,
            })
            .await;
        finish_trace(&trace, &result).await;
        drop(guard);
        result
    }

    async fn execute_traced_turn(
        &self,
        turn: TracedTurn<'_>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let TracedTurn {
            command_id,
            content,
            cancellation,
            model_id,
            reasoning,
            events,
            trace,
        } = turn;
        trace
            .record_host(
                "turn_started",
                Some("running"),
                serde_json::json!({
                    "command_id": command_id,
                    "provider": self.host.provider,
                    "model": model_id,
                    "reasoning": reasoning.as_str()
                }),
            )
            .await?;
        if let Some(outcome) = self.kernel.replay_settled_turn(command_id, &content)? {
            trace
                .record_host(
                    "durable_replay",
                    Some("completed"),
                    serde_json::json!({ "command_id": command_id }),
                )
                .await?;
            return Ok(outcome);
        }
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let model = require_model(&self.models, model_id, "active")?;
        let runtime =
            resolve_runtime(&self.host, model, reasoning, &workspace, Some(events)).await?;
        Ok(self
            .kernel
            .execute_turn(command_id, content, &runtime, cancellation)
            .await?)
    }

    /// Signals the exact active turn, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if session coordination state was poisoned.
    pub fn cancel_active_turn(&self) -> Result<(), LocalHostError> {
        if let Activity::Prompt { cancellation, .. } = &self.state()?.activity {
            cancellation.cancel();
        }
        Ok(())
    }

    /// Permanently rejects new work, cancels current work, and waits until every
    /// owned adapter has settled.
    ///
    /// # Errors
    ///
    /// Returns an error if session coordination state was poisoned.
    pub async fn close_and_wait_until_idle(&self) -> Result<(), LocalHostError> {
        {
            let mut state = self.state()?;
            state.accepting_work = false;
            if let Activity::Prompt { cancellation, .. } = &state.activity {
                cancellation.cancel();
            }
        }
        loop {
            let notified = self.idle.notified();
            if matches!(self.state()?.activity, Activity::Idle) {
                return Ok(());
            }
            notified.await;
        }
    }

    /// Selects and durably records one supported reasoning level.
    ///
    /// # Errors
    ///
    /// Rejects unsupported or concurrent changes and preserves the prior selection on failure.
    pub async fn set_reasoning(&self, reasoning: PiReasoningLevel) -> Result<(), LocalHostError> {
        let guard = self.begin_configuration()?;
        let (model_id, current) = {
            let state = self.state()?;
            (state.model.clone(), state.reasoning)
        };
        if current == reasoning {
            return Ok(());
        }
        let model = require_model(&self.models, &model_id, "active")?;
        if !model.reasoning_levels().contains(&reasoning) {
            return Err(LocalHostError::InvalidRequest(format!(
                "{model_id} does not support {} reasoning",
                reasoning.as_str()
            )));
        }
        self.validate_and_persist(model, reasoning, model_id.clone())
            .await?;
        self.state()?.reasoning = reasoning;
        drop(guard);
        Ok(())
    }

    /// Selects and durably records one discovered provider model.
    ///
    /// # Errors
    ///
    /// Rejects unknown or concurrent changes and preserves the prior selection on failure.
    pub async fn set_model(&self, model_id: &str) -> Result<(), LocalHostError> {
        let guard = self.begin_configuration()?;
        let (current_model, current_reasoning) = {
            let state = self.state()?;
            (state.model.clone(), state.reasoning)
        };
        if current_model == model_id {
            return Ok(());
        }
        let model = selected_model(&self.models, model_id)
            .ok_or_else(|| LocalHostError::InvalidRequest("unknown model selection".to_owned()))?;
        let reasoning = if model.reasoning_levels().contains(&current_reasoning) {
            current_reasoning
        } else {
            initial_reasoning(&self.models, model_id)?
        };
        self.validate_and_persist(model, reasoning, model_id.to_owned())
            .await?;
        let mut state = self.state()?;
        model_id.clone_into(&mut state.model);
        state.reasoning = reasoning;
        drop(state);
        drop(guard);
        Ok(())
    }

    fn begin_prompt(
        &self,
        request_id: Uuid,
    ) -> Result<
        (
            ActivityGuard<'_>,
            CancellationToken,
            String,
            PiReasoningLevel,
        ),
        LocalHostError,
    > {
        let mut state = self.state()?;
        if !state.accepting_work {
            return Err(LocalHostError::InvalidRequest(
                "this Alpha session is closing".to_owned(),
            ));
        }
        if !matches!(state.activity, Activity::Idle) {
            return Err(LocalHostError::InvalidRequest(
                "this Alpha session is busy".to_owned(),
            ));
        }
        let cancellation = CancellationToken::new();
        state.activity = Activity::Prompt {
            request_id,
            cancellation: cancellation.clone(),
        };
        let model = state.model.clone();
        let reasoning = state.reasoning;
        Ok((
            ActivityGuard::prompt(self, request_id),
            cancellation,
            model,
            reasoning,
        ))
    }

    fn begin_configuration(&self) -> Result<ActivityGuard<'_>, LocalHostError> {
        let mut state = self.state()?;
        if !state.accepting_work {
            return Err(LocalHostError::InvalidRequest(
                "this Alpha session is closing".to_owned(),
            ));
        }
        match state.activity {
            Activity::Idle => state.activity = Activity::Configuring,
            Activity::Prompt { .. } => {
                return Err(LocalHostError::InvalidRequest(
                    "session configuration cannot change during a prompt".to_owned(),
                ));
            }
            Activity::Configuring => {
                return Err(LocalHostError::InvalidRequest(
                    "session configuration is already changing".to_owned(),
                ));
            }
        }
        Ok(ActivityGuard::configuration(self))
    }

    async fn validate_and_persist(
        &self,
        model: &PiModelOption,
        reasoning: PiReasoningLevel,
        model_id: String,
    ) -> Result<(), LocalHostError> {
        let workspace = LocalWorkspace::open(&self.workspace)?;
        resolve_runtime(&self.host, model, reasoning, &workspace, None).await?;
        append_selection(
            self.selection_path.clone(),
            RuntimeSelection {
                provider: self.host.provider.clone(),
                model: model_id,
                reasoning,
            },
        )
        .await
    }

    fn state(&self) -> Result<MutexGuard<'_, SessionState>, LocalHostError> {
        self.state.lock().map_err(|_| LocalHostError::StatePoisoned)
    }
}

enum GuardKind {
    Prompt(Uuid),
    Configuration,
}

struct ActivityGuard<'a> {
    session: &'a AlphaSession,
    kind: GuardKind,
}

impl<'a> ActivityGuard<'a> {
    const fn prompt(session: &'a AlphaSession, request_id: Uuid) -> Self {
        Self {
            session,
            kind: GuardKind::Prompt(request_id),
        }
    }

    const fn configuration(session: &'a AlphaSession) -> Self {
        Self {
            session,
            kind: GuardKind::Configuration,
        }
    }
}

impl Drop for ActivityGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .session
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owned = match (&self.kind, &state.activity) {
            (GuardKind::Prompt(expected), Activity::Prompt { request_id, .. }) => {
                expected == request_id
            }
            (GuardKind::Configuration, Activity::Configuring) => true,
            _ => false,
        };
        if owned {
            state.activity = Activity::Idle;
            drop(state);
            self.session.idle.notify_waiters();
        }
    }
}
