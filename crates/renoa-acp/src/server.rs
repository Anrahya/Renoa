use std::sync::Arc;

use agent_client_protocol::{
    Agent, Client, ConnectionTo, JsonRpcResponse, Responder, Stdio,
    schema::{
        ProtocolVersion,
        v1::{
            AgentCapabilities, CancelNotification, Implementation, InitializeRequest,
            InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
            NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
            SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
            SetSessionConfigOptionResponse, StopReason,
        },
    },
};
use renoa_harness::OperationOutcome;
use tokio::sync::Mutex;

use crate::{Config, ServerError, events::AcpEventSink, prompt, session::ActiveSession};

pub(crate) async fn serve_stdio(config: Config) -> Result<(), ServerError> {
    let server = Arc::new(Server::new(config));
    Agent
        .builder()
        .name("renoa-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                responder.respond(initialize(request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: NewSessionRequest, responder, _connection| {
                    let server = Arc::clone(&server);
                    async move { respond(responder, server.create_session(request).await) }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = Arc::clone(&server);
                move |request: LoadSessionRequest, responder, _connection| {
                    let server = Arc::clone(&server);
                    async move { respond(responder, server.load_session(request).await) }
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
                    async move {
                        server
                            .cancel(notification)
                            .await
                            .map_err(ServerError::into_protocol_error)
                    }
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
    active: Mutex<Option<Arc<ActiveSession>>>,
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
    ) -> Result<NewSessionResponse, ServerError> {
        require_plain_local_session(&request.additional_directories, &request.mcp_servers)?;
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(session_already_active());
        }
        let session = ActiveSession::create(&self.config, &request.cwd).await?;
        let id = session.id.to_string();
        let config_options = session.config_options().await;
        *active = Some(session);
        Ok(NewSessionResponse::new(id).config_options(config_options))
    }

    async fn load_session(
        &self,
        request: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, ServerError> {
        require_plain_local_session(&request.additional_directories, &request.mcp_servers)?;
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(session_already_active());
        }
        let session = ActiveSession::load(
            &self.config,
            request.session_id.to_string().as_str(),
            &request.cwd,
        )
        .await?;
        let config_options = session.config_options().await;
        *active = Some(session);
        Ok(LoadSessionResponse::new().config_options(config_options))
    }

    async fn prompt(
        &self,
        request: PromptRequest,
        connection: &ConnectionTo<Client>,
    ) -> Result<PromptResponse, ServerError> {
        let session = self.session(&request.session_id.to_string()).await?;
        let request_id = prompt::request_identity(request.meta.as_ref())?;
        let sink = AcpEventSink::new(
            connection.clone(),
            session.id.to_string(),
            request_id.to_string(),
        );
        let outcome = prompt::execute(&session, request, request_id, &sink).await?;
        sink.ensure_delivery()?;
        match outcome {
            OperationOutcome::Completed {
                output,
                stop_reason,
                usage: _,
            } => {
                if let Some(output) = sink.remaining_chunk(&output)? {
                    connection
                        .send_notification(SessionNotification::new(
                            session.id.to_string(),
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
            OperationOutcome::Cancelled { message: _ } => {
                Ok(PromptResponse::new(StopReason::Cancelled))
            }
            OperationOutcome::Failed { message } => Err(ServerError::Operation(message)),
            _ => Err(ServerError::Operation(
                "the harness returned an unsupported operation outcome".to_owned(),
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
                session.set_model(&self.config, &value.to_string()).await?;
            }
            "thought_level" => {
                let reasoning = renoa_local::PiReasoningLevel::from_id(&value.to_string())
                    .ok_or_else(|| {
                        ServerError::InvalidRequest("unknown reasoning level".to_owned())
                    })?;
                session.set_reasoning(&self.config, reasoning).await?;
            }
            _ => {
                return Err(ServerError::InvalidRequest(
                    "unknown session configuration option".to_owned(),
                ));
            }
        }
        Ok(SetSessionConfigOptionResponse::new(
            session.config_options().await,
        ))
    }

    async fn cancel(&self, notification: CancelNotification) -> Result<(), ServerError> {
        let active = self.active.lock().await.clone();
        if let Some(session) = active
            && session.id.to_string() == notification.session_id.to_string()
        {
            session.cancel_prompt().await;
        }
        Ok(())
    }

    async fn session(&self, requested: &str) -> Result<Arc<ActiveSession>, ServerError> {
        self.active
            .lock()
            .await
            .clone()
            .filter(|session| session.id.to_string() == requested)
            .ok_or_else(|| ServerError::InvalidRequest("ACP session was not loaded".to_owned()))
    }
}

fn session_already_active() -> ServerError {
    ServerError::InvalidRequest("this ACP process already owns a session".to_owned())
}

fn initialize(_request: InitializeRequest) -> InitializeResponse {
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(
            AgentCapabilities::new()
                .load_session(true)
                .prompt_capabilities(PromptCapabilities::new().image(true)),
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
