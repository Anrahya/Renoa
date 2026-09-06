use std::sync::Arc;

use agent_client_protocol::{
    Client, ConnectionTo,
    schema::v1::{
        AvailableCommand, AvailableCommandsUpdate, CloseSessionRequest, CloseSessionResponse,
        DeleteSessionRequest, DeleteSessionResponse, LoadSessionRequest, LoadSessionResponse,
        NewSessionRequest, NewSessionResponse, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigSelectOption, SessionNotification, SessionUpdate,
    },
};
use renoa_local::{AgentSession, AgentSessionConfiguration, AgentSessionHistory};
use uuid::Uuid;

use super::{Server, require_plain_local_session};
use crate::{ServerError, events};

#[derive(Clone)]
pub(super) enum ActiveSession {
    Executable(Arc<AgentSession>),
    History {
        session: Arc<AgentSessionHistory>,
        execution_error: String,
    },
}

impl ActiveSession {
    pub(super) fn id(&self) -> Uuid {
        match self {
            Self::Executable(session) => session.id(),
            Self::History { session, .. } => session.id(),
        }
    }

    pub(super) fn executable(&self) -> Result<Arc<AgentSession>, ServerError> {
        match self {
            Self::Executable(session) => Ok(Arc::clone(session)),
            Self::History {
                execution_error, ..
            } => Err(ServerError::Operation(format!(
                "session execution is unavailable: {execution_error}; close and reload after repairing its dependencies"
            ))),
        }
    }
}

impl Server {
    pub(super) async fn create_session(
        &self,
        request: NewSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> Result<NewSessionResponse, ServerError> {
        require_plain_local_session(&request.additional_directories, &request.mcp_servers)?;
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(session_already_active());
        }
        let session = self
            .config
            .host()
            .create_session(self.config.profile_id(), &request.cwd)
            .await?;
        let id = session.id().to_string();
        let config_options = config_options(&session)?;
        send_available_commands(connection, &id)?;
        *active = Some(ActiveSession::Executable(session));
        Ok(NewSessionResponse::new(id).config_options(config_options))
    }

    pub(super) async fn load_session(
        &self,
        request: LoadSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> Result<LoadSessionResponse, ServerError> {
        require_plain_local_session(&request.additional_directories, &request.mcp_servers)?;
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(session_already_active());
        }
        let session_id = Uuid::parse_str(&request.session_id.to_string())
            .map_err(|_| ServerError::InvalidRequest("sessionId is not a Renoa UUID".to_owned()))?;
        let session = match self
            .config
            .host()
            .load_session(session_id, &request.cwd)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                let history = self
                    .config
                    .host()
                    .inspect_session(session_id, &request.cwd)
                    .await?;
                events::replay_history(
                    connection,
                    &history.id().to_string(),
                    history.history()?,
                    None,
                    None,
                )?;
                let mut meta = serde_json::Map::new();
                let execution_error = error.to_string();
                meta.insert(
                    "renoa.executionUnavailable".to_owned(),
                    serde_json::Value::String(execution_error.clone()),
                );
                if let Some(diagnostic) = history.diagnostic_error() {
                    meta.insert(
                        "renoa.traceUnavailable".to_owned(),
                        serde_json::Value::String(diagnostic.to_owned()),
                    );
                }
                *active = Some(ActiveSession::History {
                    session: Arc::new(history),
                    execution_error,
                });
                return Ok(LoadSessionResponse::new().meta(meta));
            }
        };
        let config_options = config_options(&session)?;
        events::replay_history(
            connection,
            &session.id().to_string(),
            session.history()?,
            Some(session.context_window_tokens()?),
            session.latest_context_tokens()?,
        )?;
        send_available_commands(connection, &session.id().to_string())?;
        *active = Some(ActiveSession::Executable(session));
        Ok(LoadSessionResponse::new().config_options(config_options))
    }

    pub(super) async fn close_session(
        &self,
        request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse, ServerError> {
        let requested = request.session_id.to_string();
        let session = self
            .active
            .lock()
            .await
            .clone()
            .filter(|session| session.id().to_string() == requested)
            .ok_or_else(|| ServerError::InvalidRequest("ACP session was not loaded".to_owned()))?;
        if let ActiveSession::Executable(session) = &session {
            session.close_and_wait_until_idle().await?;
        }
        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|active| active.id().to_string() == requested)
        {
            *active = None;
            Ok(CloseSessionResponse::new())
        } else {
            Err(ServerError::InvalidRequest(
                "ACP session changed while it was closing".to_owned(),
            ))
        }
    }

    pub(super) async fn delete_session(
        &self,
        request: DeleteSessionRequest,
    ) -> Result<DeleteSessionResponse, ServerError> {
        let session_id = Uuid::parse_str(&request.session_id.to_string())
            .map_err(|_| ServerError::InvalidRequest("sessionId is not a Renoa UUID".to_owned()))?;
        if self
            .active
            .lock()
            .await
            .as_ref()
            .is_some_and(|active| active.id() == session_id)
        {
            return Err(ServerError::InvalidRequest(
                "close the active ACP session before deleting it".to_owned(),
            ));
        }
        self.config.host().delete_session(session_id).await?;
        Ok(DeleteSessionResponse::new())
    }
}
pub(super) fn config_options(
    session: &AgentSession,
) -> Result<Vec<SessionConfigOption>, ServerError> {
    let AgentSessionConfiguration {
        models,
        model: selected,
        reasoning,
    } = session.configuration()?;
    let model = models
        .iter()
        .find(|model| model.selection_id() == selected)
        .ok_or_else(|| {
            ServerError::Operation("active model is absent from its catalog".to_owned())
        })?;
    Ok(vec![
        SessionConfigOption::select(
            "model",
            "Model",
            selected,
            models
                .iter()
                .map(|model| {
                    SessionConfigSelectOption::new(
                        model.selection_id(),
                        format!("{} ({})", model.name(), model.provider().name()),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .category(SessionConfigOptionCategory::Model),
        SessionConfigOption::select(
            "thought_level",
            "Reasoning",
            reasoning.as_str(),
            model
                .reasoning_levels()
                .iter()
                .map(|level| SessionConfigSelectOption::new(level.as_str(), level.name()))
                .collect::<Vec<_>>(),
        )
        .category(SessionConfigOptionCategory::ThoughtLevel),
    ])
}

fn session_already_active() -> ServerError {
    ServerError::InvalidRequest("this ACP process already owns a session".to_owned())
}

fn send_available_commands(
    connection: &ConnectionTo<Client>,
    session_id: &str,
) -> Result<(), ServerError> {
    connection
        .send_notification(SessionNotification::new(
            session_id.to_owned(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
                AvailableCommand::new("compact", "Summarize durable conversation context now"),
            ])),
        ))
        .map_err(ServerError::Transport)
}
