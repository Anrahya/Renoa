use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_client_protocol::schema::v1::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
};
use renoa_harness::{Harness, RuntimeProfile, SessionId};
use renoa_local::{LocalWorkspace, PiModelOption, PiReasoningLevel, build_local_profile};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    Config, ServerError,
    selection::{
        RuntimeSelection, SELECTION_FILE, append_selection, create_selection_log, read_selection,
    },
};

const MANIFEST_VERSION: u32 = 1;

pub(crate) struct ActiveSession {
    pub(crate) id: SessionId,
    pub(crate) harness: Arc<Harness>,
    workspace: PathBuf,
    selection_path: PathBuf,
    models: Vec<PiModelOption>,
    state: Mutex<SessionState>,
}

struct SessionState {
    profile: Arc<RuntimeProfile>,
    model: String,
    reasoning: PiReasoningLevel,
    active_prompt: Option<PromptControl>,
}

struct PromptControl {
    request_id: renoa_harness::RequestId,
    cancellation: CancellationToken,
}

pub(crate) struct PromptLease {
    pub(crate) request_id: renoa_harness::RequestId,
    pub(crate) cancellation: CancellationToken,
    pub(crate) profile: Arc<RuntimeProfile>,
}

#[derive(Serialize, Deserialize)]
struct SessionManifest {
    version: u32,
    session_id: Uuid,
    workspace: PathBuf,
}

impl ActiveSession {
    pub(crate) async fn create(config: &Config, cwd: &Path) -> Result<Arc<Self>, ServerError> {
        require_absolute(cwd)?;
        let workspace = LocalWorkspace::open(cwd)?;
        let models = config.models().await?;
        let reasoning = initial_reasoning(&models, config.model())?;
        let model = selected_model(&models, config.model())
            .expect("initial reasoning validated the configured model");
        let profile = Arc::new(
            build_local_profile(config.runtime_config(model, reasoning), &workspace).await?,
        );
        let session_uuid = Uuid::new_v4();
        let id = SessionId::from_uuid(session_uuid);
        let directory = config.sessions_directory().join(session_uuid.to_string());
        let workspace_path = std::fs::canonicalize(cwd)?;
        let selection = RuntimeSelection {
            provider: config.provider().to_owned(),
            model: config.model().to_owned(),
            reasoning,
        };
        let manifest = SessionManifest {
            version: MANIFEST_VERSION,
            session_id: session_uuid,
            workspace: workspace_path.clone(),
        };
        persist_manifest(directory.clone(), manifest).await?;
        let selection_path = directory.join(SELECTION_FILE);
        create_selection_log(selection_path.clone(), selection).await?;
        let harness = Arc::new(Harness::open(directory.join("harness.sqlite3"))?);
        harness.create_standalone_session(id).await?;
        Ok(Arc::new(Self {
            id,
            harness,
            workspace: workspace_path,
            selection_path,
            models,
            state: Mutex::new(SessionState {
                profile,
                model: config.model().to_owned(),
                reasoning,
                active_prompt: None,
            }),
        }))
    }

    pub(crate) async fn load(
        config: &Config,
        session_id: &str,
        cwd: &Path,
    ) -> Result<Arc<Self>, ServerError> {
        require_absolute(cwd)?;
        let session_uuid = Uuid::parse_str(session_id)
            .map_err(|_| ServerError::InvalidRequest("sessionId is not a Renoa UUID".to_owned()))?;
        let directory = config.sessions_directory().join(session_uuid.to_string());
        let manifest = read_manifest(directory.join("session.json")).await?;
        if manifest.version != MANIFEST_VERSION || manifest.session_id != session_uuid {
            return Err(ServerError::InvalidRequest(
                "session metadata does not match the requested session".to_owned(),
            ));
        }
        let requested_workspace = std::fs::canonicalize(cwd)?;
        if manifest.workspace != requested_workspace {
            return Err(ServerError::InvalidRequest(
                "session workspace differs from its durable binding".to_owned(),
            ));
        }
        let selection_path = directory.join(SELECTION_FILE);
        let selection = read_selection(selection_path.clone()).await?;
        if selection.provider != config.provider() {
            return Err(ServerError::Configuration(format!(
                "session requires the {} provider, but {} is configured",
                selection.provider,
                config.provider()
            )));
        }
        let models = config.models().await?;
        validate_selection(&models, &selection)?;
        let model = selected_model(&models, &selection.model)
            .expect("the saved selection was validated against its catalog");
        let workspace = LocalWorkspace::open(&requested_workspace)?;
        let profile = Arc::new(
            build_local_profile(
                config.runtime_config(model, selection.reasoning),
                &workspace,
            )
            .await?,
        );
        let id = SessionId::from_uuid(session_uuid);
        let harness = Arc::new(Harness::open(directory.join("harness.sqlite3"))?);
        harness.inspect(id).await?;
        Ok(Arc::new(Self {
            id,
            harness,
            workspace: requested_workspace,
            selection_path,
            models,
            state: Mutex::new(SessionState {
                profile,
                model: selection.model,
                reasoning: selection.reasoning,
                active_prompt: None,
            }),
        }))
    }

    pub(crate) async fn begin_prompt(
        &self,
        request_id: renoa_harness::RequestId,
    ) -> Result<PromptLease, ServerError> {
        let mut state = self.state.lock().await;
        if state.active_prompt.is_some() {
            return Err(ServerError::InvalidRequest(
                "this ACP session already has an active prompt".to_owned(),
            ));
        }
        let cancellation = CancellationToken::new();
        state.active_prompt = Some(PromptControl {
            request_id,
            cancellation: cancellation.clone(),
        });
        Ok(PromptLease {
            request_id,
            cancellation,
            profile: Arc::clone(&state.profile),
        })
    }

    pub(crate) async fn cancel_prompt(&self) {
        if let Some(active) = self.state.lock().await.active_prompt.as_ref() {
            active.cancellation.cancel();
        }
    }

    pub(crate) async fn finish_prompt(&self, request_id: renoa_harness::RequestId) {
        let mut state = self.state.lock().await;
        if state
            .active_prompt
            .as_ref()
            .is_some_and(|control| control.request_id == request_id)
        {
            state.active_prompt = None;
        }
    }

    pub(crate) async fn config_options(&self) -> Vec<SessionConfigOption> {
        let state = self.state.lock().await;
        vec![
            SessionConfigOption::select(
                "model",
                "Model",
                state.model.clone(),
                self.models
                    .iter()
                    .map(|model| {
                        SessionConfigSelectOption::new(model.id().to_owned(), model.name())
                    })
                    .collect::<Vec<_>>(),
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "thought_level",
                "Reasoning",
                state.reasoning.as_str(),
                selected_model(&self.models, &state.model)
                    .expect("active model was validated against its catalog")
                    .reasoning_levels()
                    .iter()
                    .map(|level| SessionConfigSelectOption::new(level.as_str(), level.name()))
                    .collect::<Vec<_>>(),
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ]
    }

    pub(crate) async fn set_reasoning(
        &self,
        config: &Config,
        reasoning: PiReasoningLevel,
    ) -> Result<(), ServerError> {
        let mut state = self.state.lock().await;
        if state.active_prompt.is_some() {
            return Err(ServerError::InvalidRequest(
                "session configuration cannot change during a prompt".to_owned(),
            ));
        }
        if state.reasoning == reasoning {
            return Ok(());
        }
        let model = selected_model(&self.models, &state.model)
            .expect("active model was validated against its catalog");
        if !model.reasoning_levels().contains(&reasoning) {
            return Err(ServerError::InvalidRequest(format!(
                "{} does not support {} reasoning",
                state.model,
                reasoning.as_str()
            )));
        }
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let profile =
            build_local_profile(config.runtime_config(model, reasoning), &workspace).await?;
        append_selection(
            self.selection_path.clone(),
            RuntimeSelection {
                provider: config.provider().to_owned(),
                model: state.model.clone(),
                reasoning,
            },
        )
        .await?;
        state.profile = Arc::new(profile);
        state.reasoning = reasoning;
        Ok(())
    }

    pub(crate) async fn set_model(
        &self,
        config: &Config,
        model_id: &str,
    ) -> Result<(), ServerError> {
        let mut state = self.state.lock().await;
        if state.active_prompt.is_some() {
            return Err(ServerError::InvalidRequest(
                "session configuration cannot change during a prompt".to_owned(),
            ));
        }
        if state.model == model_id {
            return Ok(());
        }
        let model = selected_model(&self.models, model_id)
            .ok_or_else(|| ServerError::InvalidRequest("unknown model selection".to_owned()))?;
        let reasoning = if model.reasoning_levels().contains(&state.reasoning) {
            state.reasoning
        } else {
            initial_reasoning(&self.models, model_id)?
        };
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let profile =
            build_local_profile(config.runtime_config(model, reasoning), &workspace).await?;
        append_selection(
            self.selection_path.clone(),
            RuntimeSelection {
                provider: config.provider().to_owned(),
                model: model_id.to_owned(),
                reasoning,
            },
        )
        .await?;
        state.profile = Arc::new(profile);
        model_id.clone_into(&mut state.model);
        state.reasoning = reasoning;
        Ok(())
    }
}

fn initial_reasoning(
    models: &[PiModelOption],
    configured_model: &str,
) -> Result<PiReasoningLevel, ServerError> {
    let model = selected_model(models, configured_model).ok_or_else(|| {
        ServerError::Configuration(format!(
            "configured {configured_model} model is not available from the authenticated provider"
        ))
    })?;
    if model.reasoning_levels().contains(&PiReasoningLevel::High) {
        return Ok(PiReasoningLevel::High);
    }
    model.reasoning_levels().first().copied().ok_or_else(|| {
        ServerError::Configuration(format!(
            "configured {configured_model} model has no supported reasoning level"
        ))
    })
}

fn selected_model<'a>(models: &'a [PiModelOption], id: &str) -> Option<&'a PiModelOption> {
    models.iter().find(|model| model.id() == id)
}

fn validate_selection(
    models: &[PiModelOption],
    selection: &RuntimeSelection,
) -> Result<(), ServerError> {
    let model = selected_model(models, &selection.model).ok_or_else(|| {
        ServerError::Configuration(format!(
            "saved {} model is no longer available",
            selection.model
        ))
    })?;
    if !model.reasoning_levels().contains(&selection.reasoning) {
        return Err(ServerError::Configuration(format!(
            "saved {} model no longer supports {} reasoning",
            selection.model,
            selection.reasoning.as_str()
        )));
    }
    Ok(())
}

fn require_absolute(cwd: &Path) -> Result<(), ServerError> {
    if cwd.is_absolute() {
        Ok(())
    } else {
        Err(ServerError::InvalidRequest(
            "session cwd must be an absolute path".to_owned(),
        ))
    }
}

async fn persist_manifest(
    directory: PathBuf,
    manifest: SessionManifest,
) -> Result<(), ServerError> {
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir(&directory)?;
        let bytes = serde_json::to_vec(&manifest)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("session.json"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        File::open(&directory)?.sync_all()?;
        Ok::<_, ServerError>(())
    })
    .await?
}

async fn read_manifest(path: PathBuf) -> Result<SessionManifest, ServerError> {
    let bytes = tokio::fs::read(path).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
