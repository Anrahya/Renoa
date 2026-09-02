use std::{
    num::NonZeroU64,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentProfile, AgentProfileId, LocalHistoryEntry, LocalHostError, LocalSession, LocalWorkspace,
    ModelChoice, ModelProvider, ReasoningLevel,
    host::{
        HostConfig, RuntimeRequest, discover_profile_models, initial_reasoning, require_model,
        resolve_runtime, selected_model_by_selection_id,
    },
    selection::{RuntimeSelection, append_selection},
    trace::TraceStore,
};

mod execution;

/// One durable Agent session assembled from an exact Host profile.
pub struct AgentSession {
    id: Uuid,
    profile_id: AgentProfileId,
    host: Arc<HostConfig>,
    kernel: LocalSession,
    workspace: PathBuf,
    selection_path: PathBuf,
    trace: TraceStore,
    state: Mutex<SessionState>,
    idle: tokio::sync::Notify,
}

/// Durable resources belonging to one assembled Agent session.
pub(crate) struct AgentSessionStorage {
    pub(crate) kernel: LocalSession,
    pub(crate) workspace: PathBuf,
    pub(crate) selection_path: PathBuf,
    pub(crate) trace: TraceStore,
}

struct SessionState {
    models: Vec<ModelChoice>,
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
pub struct AgentSessionConfiguration {
    pub models: Vec<ModelChoice>,
    pub model: String,
    pub reasoning: ReasoningLevel,
}

impl AgentSession {
    pub(crate) fn new(
        id: Uuid,
        profile_id: AgentProfileId,
        host: Arc<HostConfig>,
        storage: AgentSessionStorage,
        models: Vec<ModelChoice>,
        selection: RuntimeSelection,
    ) -> Self {
        Self {
            id,
            profile_id,
            host,
            kernel: storage.kernel,
            workspace: storage.workspace,
            selection_path: storage.selection_path,
            trace: storage.trace,
            state: Mutex::new(SessionState {
                models,
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

    #[must_use]
    pub const fn profile_id(&self) -> &AgentProfileId {
        &self.profile_id
    }

    #[must_use]
    pub const fn agent_id(&self) -> renoa_kernel::AgentId {
        self.kernel.agent_id()
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
    pub fn configuration(&self) -> Result<AgentSessionConfiguration, LocalHostError> {
        let state = self.state()?;
        let model = require_model(&state.models, state.provider, &state.model, "active")?;
        Ok(AgentSessionConfiguration {
            models: state.models.clone(),
            model: model.selection_id(),
            reasoning: state.reasoning,
        })
    }

    /// Refreshes this profile's provider catalog without changing the active selection.
    ///
    /// # Errors
    ///
    /// Rejects concurrent work and preserves the previous catalog if discovery or
    /// validation fails.
    pub async fn refresh_configuration(&self) -> Result<AgentSessionConfiguration, LocalHostError> {
        let guard = self.begin_configuration()?;
        let (provider, model_id, reasoning) = {
            let state = self.state()?;
            (state.provider, state.model.clone(), state.reasoning)
        };
        let profile = self.profile()?.clone();
        let models = discover_profile_models(&self.host, &profile).await?;
        let model = require_model(&models, provider, &model_id, "active")?;
        if !model.reasoning_levels().contains(&reasoning) {
            return Err(LocalHostError::Configuration(format!(
                "active {model_id} model no longer supports {} reasoning",
                reasoning.as_str()
            )));
        }
        let configuration = AgentSessionConfiguration {
            models: models.clone(),
            model: model.selection_id(),
            reasoning,
        };
        self.state()?.models = models;
        drop(guard);
        Ok(configuration)
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
            require_model(&state.models, state.provider, &state.model, "active")?
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
        let (model, current) = {
            let state = self.state()?;
            (
                require_model(&state.models, state.provider, &state.model, "active")?.clone(),
                state.reasoning,
            )
        };
        if current == reasoning {
            return Ok(());
        }
        if !model.reasoning_levels().contains(&reasoning) {
            return Err(LocalHostError::InvalidRequest(format!(
                "{} does not support {} reasoning",
                model.id(),
                reasoning.as_str()
            )));
        }
        self.validate_and_persist(&model, reasoning, model.id().to_owned())
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
        let (current_selection, current_reasoning) = {
            let state = self.state()?;
            (
                require_model(&state.models, state.provider, &state.model, "active")?
                    .selection_id(),
                state.reasoning,
            )
        };
        if current_selection == model_id {
            return Ok(());
        }
        let profile = self.profile()?.clone();
        let models = discover_profile_models(&self.host, &profile).await?;
        let model = if let Some(model) = selected_model_by_selection_id(&models, model_id) {
            model.clone()
        } else {
            let mut matching = models.iter().filter(|model| model.id() == model_id);
            let model = matching.next().ok_or_else(|| {
                LocalHostError::InvalidRequest(format!(
                    "model `{model_id}` is not available for this agent profile"
                ))
            })?;
            if matching.next().is_some() {
                return Err(LocalHostError::InvalidRequest(format!(
                    "model id `{model_id}` exists under more than one provider; use its provider-qualified id"
                )));
            }
            model.clone()
        };
        let reasoning = if model.reasoning_levels().contains(&current_reasoning) {
            current_reasoning
        } else {
            initial_reasoning(&models, model.provider(), model.id())?
        };
        self.validate_and_persist(&model, reasoning, model.id().to_owned())
            .await?;
        let mut state = self.state()?;
        state.models = models;
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
            ModelChoice,
            ReasoningLevel,
        ),
        LocalHostError,
    > {
        let mut state = self.state()?;
        if !state.accepting_work {
            return Err(LocalHostError::InvalidRequest(
                "this Agent session is closing".to_owned(),
            ));
        }
        if !matches!(state.activity, Activity::Idle) {
            return Err(LocalHostError::InvalidRequest(
                "this Agent session is busy".to_owned(),
            ));
        }
        let model = require_model(&state.models, state.provider, &state.model, "active")?.clone();
        let cancellation = CancellationToken::new();
        state.activity = Activity::Prompt {
            request_id,
            cancellation: cancellation.clone(),
        };
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
                "this Agent session is closing".to_owned(),
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
        let profile = self.profile()?.clone();
        resolve_runtime(
            &self.host,
            RuntimeRequest {
                profile: &profile,
                session_id: renoa_kernel::SessionId::from_uuid(self.id),
                command_id: None,
                model,
                reasoning,
                workspace: &workspace,
                events: None,
            },
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

    fn profile(&self) -> Result<&AgentProfile, LocalHostError> {
        self.host.profiles.get(&self.profile_id).ok_or_else(|| {
            LocalHostError::InvalidRequest(format!(
                "agent profile `{}` is no longer registered with this Host",
                self.profile_id
            ))
        })
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
    session: &'a AgentSession,
    kind: GuardKind,
}

impl<'a> ActivityGuard<'a> {
    const fn prompt(session: &'a AgentSession, request_id: Uuid) -> Self {
        Self {
            session,
            kind: GuardKind::Prompt(request_id),
        }
    }

    const fn configuration(session: &'a AgentSession) -> Self {
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
