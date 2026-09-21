use std::sync::Arc;

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::AgentId;
use renoa_local::{AgentSession, LocalHost, LocalTurnOutcome, TurnObservation};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    gateway,
    ingress::pages,
    snowflake::Snowflake,
    store::{Outbound, QueuedTurn, SurfaceStore},
};

struct Quiet;

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

pub(crate) struct Surface {
    pub(crate) host: LocalHost,
    pub(crate) agent_id: Uuid,
    pub(crate) workspace: std::path::PathBuf,
    pub(crate) guild_id: Snowflake,
    pub(crate) operator_user_id: Snowflake,
    pub(crate) token: String,
    pub(crate) data_directory: std::path::PathBuf,
}

pub(crate) async fn run(
    surface: Surface,
    api: DiscordApi,
    shutdown: CancellationToken,
) -> Result<(), DiscordError> {
    let agent_id = AgentId::from_uuid(surface.agent_id);
    if surface.host.agent_definition(agent_id).await?.is_none() {
        return Err(DiscordError::Invalid(format!(
            "configured agent {} is not provisioned on this Host",
            surface.agent_id
        )));
    }
    let store = SurfaceStore::open(&surface.data_directory)?;
    store.bind_identity(
        &surface.guild_id,
        &surface.operator_user_id,
        surface.agent_id,
    )?;
    store.recover()?;
    let store = Arc::new(store);
    let wake = Arc::new(Notify::new());
    let host = Arc::new(surface.host);
    let api = Arc::new(api);
    let mut tasks = tokio::task::JoinSet::new();
    let gateway_store = Arc::clone(&store);
    let gateway_wake = Arc::clone(&wake);
    let gateway_shutdown = shutdown.clone();
    let gateway_api = Arc::clone(&api);
    let guild_id = surface.guild_id.clone();
    let operator_user_id = surface.operator_user_id.clone();
    let token = surface.token.clone();
    tasks.spawn(async move {
        gateway::maintain(
            &gateway_api,
            &gateway_store,
            &gateway_wake,
            &gateway_shutdown,
            &guild_id,
            &operator_user_id,
            &token,
        )
        .await
    });
    let worker_store = Arc::clone(&store);
    let worker_wake = Arc::clone(&wake);
    let worker_shutdown = shutdown.clone();
    let worker_api = Arc::clone(&api);
    let workspace = surface.workspace.clone();
    tasks.spawn(async move {
        worker(
            host,
            agent_id,
            workspace,
            worker_api,
            worker_store,
            worker_wake,
            worker_shutdown,
        )
        .await
    });
    let first = tokio::select! {
        () = shutdown.cancelled() => None,
        result = tasks.join_next() => result,
    };
    shutdown.cancel();
    let mut failure = match first {
        Some(Ok(result)) => result.err(),
        Some(Err(error)) => Some(DiscordError::Task(error)),
        None => None,
    };
    while let Some(result) = tasks.join_next().await {
        if failure.is_none()
            && let Ok(Err(error)) = result
        {
            failure = Some(error);
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn worker(
    host: Arc<LocalHost>,
    agent_id: AgentId,
    workspace: std::path::PathBuf,
    api: Arc<DiscordApi>,
    store: Arc<SurfaceStore>,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
) -> Result<(), DiscordError> {
    let mut session: Option<(Uuid, Arc<AgentSession>)> = None;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        if let Some(outbound) = store.next_outbound()? {
            deliver(&api, &store, &outbound, &shutdown).await?;
            continue;
        }
        if let Some(turn) = store.next_queued()? {
            let held =
                session_for(&host, agent_id, &workspace, &mut session, turn.session_id).await?;
            execute(&store, &held, &turn, &shutdown).await?;
            continue;
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = wake.notified() => {}
        }
    }
}

async fn session_for(
    host: &LocalHost,
    agent_id: AgentId,
    workspace: &std::path::Path,
    cached: &mut Option<(Uuid, Arc<AgentSession>)>,
    session_id: Uuid,
) -> Result<Arc<AgentSession>, DiscordError> {
    if let Some((cached_id, session)) = cached
        && *cached_id == session_id
    {
        return Ok(Arc::clone(session));
    }
    let session = host
        .ensure_agent_session(agent_id, workspace, session_id)
        .await?;
    *cached = Some((session_id, Arc::clone(&session)));
    Ok(session)
}

async fn execute(
    store: &SurfaceStore,
    session: &AgentSession,
    turn: &QueuedTurn,
    shutdown: &CancellationToken,
) -> Result<(), DiscordError> {
    store.mark_running(&turn.message_id)?;
    if turn.prompt.is_empty() {
        let text = "Send a task after the mention.".to_owned();
        store.mark_ready(&turn.message_id, &text, &pages(&text))?;
        return Ok(());
    }
    let observation = TurnObservation::from_unix_milliseconds(turn.observed_at_ms)
        .map_err(|error| DiscordError::Invalid(error.to_string()))?;
    let outcome = session
        .execute_turn_observed_with_cancellation(
            turn.request_id,
            vec![ContentBlock::text(turn.prompt.clone())],
            observation,
            Arc::new(Quiet),
            shutdown.clone(),
        )
        .await;
    let text = match outcome {
        Ok(outcome) => reply_text(outcome),
        Err(error) => format!("The agent could not complete this turn: {error}"),
    };
    let reply_pages = pages(&text);
    store.mark_ready(&turn.message_id, &text, &reply_pages)?;
    Ok(())
}

fn reply_text(outcome: LocalTurnOutcome) -> String {
    match outcome {
        LocalTurnOutcome::Completed { output, .. } => output,
        LocalTurnOutcome::Cancelled => "Stopped.".to_owned(),
        LocalTurnOutcome::Failed { reason } => {
            format!("The agent could not complete this turn: {reason}")
        }
        LocalTurnOutcome::Compacted {
            estimated_input_tokens,
        } => format!("Context compacted. Estimated context: {estimated_input_tokens} tokens."),
        LocalTurnOutcome::WaitingForInput => "The agent is waiting for more input.".to_owned(),
        _ => "The agent returned an outcome this Discord surface does not post.".to_owned(),
    }
}

async fn deliver(
    api: &DiscordApi,
    store: &SurfaceStore,
    outbound: &Outbound,
    shutdown: &CancellationToken,
) -> Result<(), DiscordError> {
    store.mark_sending(&outbound.message_id, outbound.chunk)?;
    let reply_to = (outbound.chunk == 0).then_some(outbound.message_id.as_str());
    match api
        .create_message(&outbound.channel_id, &outbound.body, reply_to)
        .await
    {
        Ok(reply_id) => {
            let reply_id = Snowflake::parse(&reply_id)?;
            store.mark_sent(&outbound.message_id, outbound.chunk, &reply_id)?;
            Ok(())
        }
        Err(ApiError::RateLimited(delay)) => {
            store.release_sending(&outbound.message_id, outbound.chunk)?;
            tokio::select! {
                () = shutdown.cancelled() => Ok(()),
                () = tokio::time::sleep(delay) => Ok(()),
            }
        }
        Err(ApiError::Unknown(_)) => {
            store.mark_unknown(&outbound.message_id, outbound.chunk)?;
            Ok(())
        }
        Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized.into()),
        Err(error) => {
            eprintln!("renoa-discord: reply rejected: {error}");
            store.mark_failed(&outbound.message_id, outbound.chunk)?;
            Ok(())
        }
    }
}
