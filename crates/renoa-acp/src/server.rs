use std::sync::Arc;

use agent_client_protocol::{
    Agent, Client, ConnectionTo, JsonRpcResponse, Responder, Stdio,
    schema::{
        ProtocolVersion,
        v1::{
            AgentCapabilities, AvailableCommand, AvailableCommandsUpdate, CancelNotification,
            CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest, DeleteSessionResponse,
            Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest,
            LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptCapabilities,
            PromptRequest, PromptResponse, SessionCapabilities, SessionCloseCapabilities,
            SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
            SessionDeleteCapabilities, SessionNotification, SessionUpdate,
            SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
        },
    },
};
use renoa_agent::AgentEventSink;
use renoa_local::{AlphaSession, AlphaSessionConfiguration, LocalTurnOutcome};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    Config, ServerError,
    events::{self, AcpEventSink},
    prompt,
};

pub(crate) async fn serve_stdio(config: Config) -> Result<(), ServerError> {
    let server = Arc::new(Server::new(config));
    Agent
        .builder()
        .name("renoa-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| responder
                .respond(initialize(request)),
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: CloseSessionRequest, responder, _connection| {
                    let server = Arc::clone(&server);
                    async move { respond(responder, server.close_session(request).await) }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: DeleteSessionRequest, responder, _connection| {
                    let server = Arc::clone(&server);
                    async move { respond(responder, server.delete_session(request).await) }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: NewSessionRequest, responder, connection| {
                    let server = Arc::clone(&server);
                    async move {
                        respond(responder, server.create_session(request, &connection).await)
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: LoadSessionRequest, responder, connection| {
                    let server = Arc::clone(&server);
                    async move {
                        respond(
                            responder,
                            server.load_session(request, &connection).await,
                        )
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: PromptRequest,
                      responder: Responder<PromptResponse>,
                      connection: ConnectionTo<Client>| {
                    let server = Arc::clone(&server);
                    async move {
                        let task_connection = connection.clone();
                        connection.spawn(async move {
                            respond(responder, server.prompt(request, &task_connection).await)
                        })
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: SetSessionConfigOptionRequest, responder, _connection| {
                    let server = Arc::clone(&server);
                    async move { respond(responder, server.set_config_option(request).await) }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let server = Arc::clone(&server);
                move |notification: CancelNotification, _connection| {
                    let server = Arc::clone(&server);
                    async move { server.cancel(notification).await.map_err(ServerError::into_protocol_error) }
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(ServerError::Transport)
}

struct Server {
    config: Config,
    active: Mutex<Option<Arc<AlphaSession>>>,
}

impl Server {
    fn new(config: Config) -> Self {
        Self {
            config,
            active: Mutex::new(None),
        }
    }

    async fn create_session(
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
            .create_alpha_session(&request.cwd)
            .await?;
        let id = session.id().to_string();
        let config_options = config_options(&session)?;
        send_available_commands(connection, &id)?;
        *active = Some(session);
        Ok(NewSessionResponse::new(id).config_options(config_options))
    }

    async fn load_session(
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
        let session = self
            .config
            .host()
            .load_alpha_session(session_id, &request.cwd)
            .await?;
        let config_options = config_options(&session)?;
        events::replay_history(
            connection,
            &session.id().to_string(),
            session.history()?,
            session.context_window_tokens()?,
            session.latest_context_tokens()?,
        )?;
        send_available_commands(connection, &session.id().to_string())?;
        *active = Some(session);
        Ok(LoadSessionResponse::new().config_options(config_options))
    }

    async fn close_session(
        &self,
        request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse, ServerError> {
        let requested = request.session_id.to_string();
        let session = self.session(&requested).await?;
        session.close_and_wait_until_idle().await?;
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

    async fn delete_session(
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
        self.config.host().delete_alpha_session(session_id).await?;
        Ok(DeleteSessionResponse::new())
    }

    async fn prompt(
        &self,
        request: PromptRequest,
        connection: &ConnectionTo<Client>,
    ) -> Result<PromptResponse, ServerError> {
        let session = self.session(&request.session_id.to_string()).await?;
        let request_id = prompt::request_identity(request.meta.as_ref())?;
        let action = prompt::action(request.prompt)?;
        if matches!(action, prompt::PromptAction::Compact) {
            let context_window_tokens = session.context_window_tokens()?;
            let outcome =
                prompt::execute(&session, action, request_id, prompt::silent_sink()).await?;
            return match outcome {
                LocalTurnOutcome::Compacted {
                    estimated_input_tokens,
                } => {
                    events::send_context_usage(
                        connection,
                        &session.id().to_string(),
                        estimated_input_tokens,
                        context_window_tokens,
                    )?;
                    Ok(PromptResponse::new(StopReason::EndTurn))
                }
                LocalTurnOutcome::Cancelled => Ok(PromptResponse::new(StopReason::Cancelled)),
                LocalTurnOutcome::Failed { reason } => Err(ServerError::Operation(reason)),
                LocalTurnOutcome::WaitingForInput => Err(ServerError::Operation(
                    "explicit compaction is waiting for unsupported external input".to_owned(),
                )),
                LocalTurnOutcome::Completed { .. } => Err(ServerError::Operation(
                    "explicit compaction returned an assistant response".to_owned(),
                )),
                _ => Err(ServerError::Operation(
                    "explicit compaction returned an unsupported outcome".to_owned(),
                )),
            };
        }
        let sink = Arc::new(AcpEventSink::new(
            connection.clone(),
            session.id().to_string(),
            session.context_window_tokens()?,
        ));
        let events: Arc<dyn AgentEventSink> = sink.clone();
        let outcome = prompt::execute(&session, action, request_id, events).await?;
        sink.ensure_delivery()?;
        match outcome {
            LocalTurnOutcome::Completed {
                output,
                stop_reason,
            } => {
                if let Some(output) = sink.remaining_chunk(&output)? {
                    connection
                        .send_notification(SessionNotification::new(
                            session.id().to_string(),
                            SessionUpdate::AgentMessageChunk(output),
                        ))
                        .map_err(ServerError::Transport)?;
                }
                Ok(PromptResponse::new(match stop_reason {
                    renoa_agent::StopReason::Length => StopReason::MaxTokens,
                    renoa_agent::StopReason::Stop | renoa_agent::StopReason::ToolUse => {
                        StopReason::EndTurn
                    }
                }))
            }
            LocalTurnOutcome::Cancelled => Ok(PromptResponse::new(StopReason::Cancelled)),
            LocalTurnOutcome::Failed { reason } => match sink.last_model_failure()? {
                Some(failure) if failure.cancelled => {
                    Ok(PromptResponse::new(StopReason::Cancelled))
                }
                Some(failure) => Err(ServerError::Operation(failure.message)),
                None => Err(ServerError::Operation(reason)),
            },
            LocalTurnOutcome::WaitingForInput => Err(ServerError::Operation(
                "the Alpha coding turn is waiting for unsupported external input".to_owned(),
            )),
            LocalTurnOutcome::Compacted { .. } => Err(ServerError::Operation(
                "a normal prompt returned a compaction result".to_owned(),
            )),
            _ => Err(ServerError::Operation(
                "the local Host returned an unsupported turn outcome".to_owned(),
            )),
        }
    }

    async fn set_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse, ServerError> {
        let session = self.session(&request.session_id.to_string()).await?;
        let value = request.value.as_value_id().ok_or_else(|| {
            ServerError::InvalidRequest(
                "session configuration value must be a selector id".to_owned(),
            )
        })?;
        match request.config_id.to_string().as_str() {
            "model" => {
                session.set_model(&value.to_string()).await?;
            }
            "thought_level" => {
                let reasoning = renoa_local::ReasoningLevel::from_id(&value.to_string())
                    .ok_or_else(|| {
                        ServerError::InvalidRequest("unknown reasoning level".to_owned())
                    })?;
                session.set_reasoning(reasoning).await?;
            }
            _ => {
                return Err(ServerError::InvalidRequest(
                    "unknown session configuration option".to_owned(),
                ));
            }
        }
        Ok(SetSessionConfigOptionResponse::new(config_options(
            &session,
        )?))
    }

    async fn cancel(&self, notification: CancelNotification) -> Result<(), ServerError> {
        let active = self.active.lock().await.clone();
        if let Some(session) = active
            && session.id().to_string() == notification.session_id.to_string()
        {
            session.cancel_active_turn()?;
        }
        Ok(())
    }

    async fn session(&self, requested: &str) -> Result<Arc<AlphaSession>, ServerError> {
        self.active
            .lock()
            .await
            .clone()
            .filter(|session| session.id().to_string() == requested)
            .ok_or_else(|| ServerError::InvalidRequest("ACP session was not loaded".to_owned()))
    }
}

fn config_options(session: &AlphaSession) -> Result<Vec<SessionConfigOption>, ServerError> {
    let AlphaSessionConfiguration {
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

fn initialize(_request: InitializeRequest) -> InitializeResponse {
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(
            AgentCapabilities::new()
                .load_session(true)
                .prompt_capabilities(PromptCapabilities::new().image(true))
                .session_capabilities(
                    SessionCapabilities::new()
                        .close(SessionCloseCapabilities::new())
                        .delete(SessionDeleteCapabilities::new()),
                ),
        )
        .agent_info(Implementation::new(
            "renoa-agent",
            env!("CARGO_PKG_VERSION"),
        ))
}

fn require_plain_local_session<T>(
    additional_directories: &[std::path::PathBuf],
    mcp_servers: &[T],
) -> Result<(), ServerError> {
    if !additional_directories.is_empty() {
        return Err(ServerError::InvalidRequest(
            "additional workspace directories are not supported yet".to_owned(),
        ));
    }
    if !mcp_servers.is_empty() {
        return Err(ServerError::InvalidRequest(
            "MCP servers are not supported yet".to_owned(),
        ));
    }
    Ok(())
}

fn respond<T: JsonRpcResponse>(
    responder: Responder<T>,
    response: Result<T, ServerError>,
) -> Result<(), agent_client_protocol::Error> {
    responder.respond_with_result(response.map_err(ServerError::into_protocol_error))
}
