use std::{
    num::NonZeroU64,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    LocalHistoryEntry, LocalHostError, LocalSession, LocalWorkspace, ModelChoice, ModelProvider,
    ReasoningLevel,
    host::{
        HostConfig, initial_reasoning, require_model, resolve_runtime,
        selected_model_by_selection_id,
    },
    selection::{RuntimeSelection, append_selection},
    trace::TraceStore,
};

mod execution;

/// One durable Alpha Agent instance assembled by the local Host.
pub struct AlphaSession {
    id: Uuid,
    host: Arc<HostConfig>,
    kernel: LocalSession,
    workspace: PathBuf,
    selection_path: PathBuf,
    trace: TraceStore,
    models: Vec<ModelChoice>,
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
    provider: ModelProvider,
    model: String,
    reasoning: ReasoningLevel,
    accepting_work: bool,
    activity: Activity,
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
    pub models: Vec<ModelChoice>,
    pub model: String,
    pub reasoning: ReasoningLevel,
}

impl AlphaSession {
    pub(crate) fn new(
        id: Uuid,
        host: Arc<HostConfig>,
        storage: AlphaSessionStorage,
        models: Vec<ModelChoice>,
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
                provider: selection.provider,
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
        let model = require_model(&self.models, state.provider, &state.model, "active")?;
        Ok(AlphaSessionConfiguration {
            models: self.models.clone(),
            model: model.selection_id(),
            reasoning: state.reasoning,
        })
    }

    /// Returns the active model's advertised context-window size.
    ///
    /// # Errors
    ///
    /// Returns an error if session coordination state was poisoned or its
    /// persisted model no longer exists in the authenticated catalog.
    pub fn context_window_tokens(&self) -> Result<NonZeroU64, LocalHostError> {
        let state = self.state()?;
        Ok(
            require_model(&self.models, state.provider, &state.model, "active")?
                .context_window_tokens(),
        )
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
    pub async fn set_reasoning(&self, reasoning: ReasoningLevel) -> Result<(), LocalHostError> {
        let guard = self.begin_configuration()?;
        let (provider, model_id, current) = {
            let state = self.state()?;
            (state.provider, state.model.clone(), state.reasoning)
        };
        if current == reasoning {
            return Ok(());
        }
        let model = require_model(&self.models, provider, &model_id, "active")?;
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
        let (current_provider, current_model, current_reasoning) = {
            let state = self.state()?;
            (state.provider, state.model.clone(), state.reasoning)
        };
        let current = require_model(&self.models, current_provider, &current_model, "active")?;
        if current.selection_id() == model_id {
            return Ok(());
        }
        let model = selected_model_by_selection_id(&self.models, model_id)
            .ok_or_else(|| LocalHostError::InvalidRequest("unknown model selection".to_owned()))?;
        let reasoning = if model.reasoning_levels().contains(&current_reasoning) {
            current_reasoning
        } else {
            initial_reasoning(&self.models, model.provider(), model.id())?
        };
        self.validate_and_persist(model, reasoning, model.id().to_owned())
            .await?;
        let mut state = self.state()?;
        state.provider = model.provider();
        model.id().clone_into(&mut state.model);
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
            ModelProvider,
            String,
            ReasoningLevel,
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
            state.provider,
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
        model: &ModelChoice,
        reasoning: ReasoningLevel,
        model_id: String,
    ) -> Result<(), LocalHostError> {
        let workspace = LocalWorkspace::open(&self.workspace)?;
        resolve_runtime(
            &self.host,
            renoa_kernel::SessionId::from_uuid(self.id),
            None,
            model,
            reasoning,
            &workspace,
            None,
        )
        .await?;
        append_selection(
            self.selection_path.clone(),
            RuntimeSelection {
                provider: model.provider(),
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
