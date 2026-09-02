use std::{collections::HashMap, sync::Arc, time::Duration};

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_local::{AgentProfileId, AgentSession, LocalHost, LocalTurnOutcome, TurnObservation};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    Config, TelegramServiceError,
    api::{ApiError, TelegramApi},
    delivery::{self, DeliveryProgress},
    events::SurfaceEvents,
    ingress::{self, Topic},
    log,
    store::{ImmediateAction, PendingAction, SurfaceStore, WorkItem, WorkKind},
};

mod commands;

/// Runs the supervised Arcee Telegram surface until shutdown or a required task fails.
///
/// # Errors
///
/// Returns configuration, Telegram transport, Host, durable state, or supervision failures.
pub async fn run(config: Config) -> Result<(), TelegramServiceError> {
    let api = Arc::new(TelegramApi::new(
        &config.bot_token,
        config.telegram_ipv4_only,
    )?);
    let bot = api.get_me().await?;
    if !bot.is_bot || bot.id <= 0 {
        return Err(TelegramServiceError::Configuration(
            "the configured Telegram credential does not identify a bot".to_owned(),
        ));
    }
    api.require_long_polling().await?;
    let store = SurfaceStore::open(&config.data_directory)?;
    store
        .bind_identity(bot.id, config.allowed_user_id, &config.workspace)
        .await?;
    let recovery = store.recover().await?;
    api.set_commands().await?;
    log::event(
        "info",
        "surface_started",
        &serde_json::json!({
            "bot_id": bot.id,
            "profile_id": config.profile_id.as_str(),
            "requeued": recovery.requeued,
            "delivery_unknown": recovery.delivery_unknown,
            "action_delivery_unknown": recovery.action_delivery_unknown,
        }),
    );

    let active = Arc::new(ActiveTurn::default());
    let wake = Arc::new(Notify::new());
    let shutdown = CancellationToken::new();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(poll_updates(Poller {
        api: Arc::clone(&api),
        store: store.clone(),
        allowed_user_id: config.allowed_user_id,
        bot_username: bot.username,
        active: Arc::clone(&active),
        wake: Arc::clone(&wake),
        shutdown: shutdown.clone(),
    }));
    tasks.spawn(run_worker(Worker {
        api,
        store,
        host: Arc::new(config.host),
        profile_id: config.profile_id,
        workspace: config.workspace,
        sessions: HashMap::new(),
        active: Arc::clone(&active),
        wake,
        shutdown: shutdown.clone(),
    }));

    let first = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal?;
            None
        }
        result = tasks.join_next() => result,
    };
    shutdown.cancel();
    active.cancel_any().await;
    let first_result = match first {
        Some(Ok(result)) => Some(result),
        Some(Err(error)) => Some(Err(TelegramServiceError::Task(error))),
        None => None,
    };
    while let Some(result) = tasks.join_next().await {
        let error = match result {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => error.to_string(),
            Err(error) => error.to_string(),
        };
        log::event(
            "error",
            "surface_task_failed_during_shutdown",
            &serde_json::json!({"error": error}),
        );
    }
    match first_result {
        None => Ok(()),
        Some(Ok(())) => Err(TelegramServiceError::Supervision(
            "a required surface task exited unexpectedly".to_owned(),
        )),
        Some(Err(error)) => Err(error),
    }
}

struct Poller {
    api: Arc<TelegramApi>,
    store: SurfaceStore,
    allowed_user_id: i64,
    bot_username: Option<String>,
    active: Arc<ActiveTurn>,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
}

async fn poll_updates(poller: Poller) -> Result<(), TelegramServiceError> {
    let mut failures = 0_u32;
    loop {
        let offset = poller.store.next_update_id().await?;
        let received = tokio::select! {
            () = poller.shutdown.cancelled() => return Ok(()),
            result = poller.api.updates(offset) => result,
        };
        match received {
            Ok(updates) => {
                failures = 0;
                for update in updates {
                    let parsed = ingress::parse(
                        update,
                        poller.allowed_user_id,
                        poller.bot_username.as_deref(),
                    );
                    let update_id = parsed.update_id;
                    let kind = parsed.kind.storage_name();
                    let admission = poller.store.admit(parsed).await?;
                    if let Some(immediate) = admission.immediate {
                        apply_immediate(&poller.active, immediate).await?;
                    }
                    if admission.queued {
                        poller.wake.notify_one();
                    }
                    if !admission.duplicate {
                        log::event(
                            "info",
                            "update_admitted",
                            &serde_json::json!({"update_id": update_id, "kind": kind}),
                        );
                    }
                }
            }
            Err(error) if error.is_fatal_polling_error() => return Err(error.into()),
            Err(error) => {
                failures = failures.saturating_add(1);
                let delay = retry_delay(&error, failures);
                log::event(
                    "warn",
                    "poll_retry",
                    &serde_json::json!({
                        "attempt": failures,
                        "delay_ms": delay.as_millis(),
                        "error": error.to_string(),
                    }),
                );
                tokio::select! {
                    () = poller.shutdown.cancelled() => return Ok(()),
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

async fn apply_immediate(
    active: &ActiveTurn,
    action: ImmediateAction,
) -> Result<(), TelegramServiceError> {
    match action {
        ImmediateAction::Cancel(topic) => {
            active.cancel(topic, None).await?;
        }
        ImmediateAction::Stop { topic, draft_id } => {
            active.cancel(topic, Some(draft_id)).await?;
        }
    }
    Ok(())
}

struct Worker {
    api: Arc<TelegramApi>,
    store: SurfaceStore,
    host: Arc<LocalHost>,
    profile_id: AgentProfileId,
    workspace: std::path::PathBuf,
    sessions: HashMap<Uuid, Arc<AgentSession>>,
    active: Arc<ActiveTurn>,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
}

async fn run_worker(mut worker: Worker) -> Result<(), TelegramServiceError> {
    loop {
        if worker.shutdown.is_cancelled() {
            return Ok(());
        }
        match worker.store.next_action().await? {
            Some(PendingAction::Execute(item)) => worker.execute(item).await?,
            Some(PendingAction::Deliver(item)) => {
                if let DeliveryProgress::RetryAfter(delay) =
                    delivery::deliver(&worker.api, &worker.store, item).await?
                {
                    tokio::select! {
                        () = worker.shutdown.cancelled() => return Ok(()),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
            }
            None => {
                tokio::select! {
                    () = worker.shutdown.cancelled() => return Ok(()),
                    () = worker.wake.notified() => {}
                }
            }
        }
    }
}

impl Worker {
    async fn execute(&mut self, item: WorkItem) -> Result<(), TelegramServiceError> {
        self.store.mark_running(item.update_id).await?;
        log::event(
            "info",
            "command_started",
            &serde_json::json!({
                "update_id": item.update_id,
                "request_id": item.request_id,
                "session_id": item.session_id,
            }),
        );
        let result = match &item.kind {
            WorkKind::Notice(text) => text.clone(),
            WorkKind::New => match self.session(item.session_id).await {
                Ok(_) => "Started a fresh Arcee session in this topic.".to_owned(),
                Err(error) => surface_error(&error),
            },
            WorkKind::Status => match self.session(item.session_id).await {
                Ok(session) => {
                    session_status(&session).unwrap_or_else(|error| surface_error(&error))
                }
                Err(error) => surface_error(&error),
            },
            WorkKind::Model(requested) => match self.session(item.session_id).await {
                Ok(session) => commands::model(&session, requested.as_deref())
                    .await
                    .unwrap_or_else(|error| surface_error(&error)),
                Err(error) => surface_error(&error),
            },
            WorkKind::Reasoning(requested) => match self.session(item.session_id).await {
                Ok(session) => commands::reasoning(&session, requested.as_deref())
                    .await
                    .unwrap_or_else(|error| surface_error(&error)),
                Err(error) => surface_error(&error),
            },
            WorkKind::Cancel => "Stop request processed.".to_owned(),
            WorkKind::Prompt(text) => self.run_agent(&item, Some(text)).await?,
            WorkKind::Compact => self.run_agent(&item, None).await?,
        };
        self.store.set_result(item.update_id, result).await?;
        log::event(
            "info",
            "command_settled",
            &serde_json::json!({"update_id": item.update_id, "request_id": item.request_id}),
        );
        Ok(())
    }

    async fn run_agent(
        &mut self,
        item: &WorkItem,
        prompt: Option<&str>,
    ) -> Result<String, TelegramServiceError> {
        let session = match self.session(item.session_id).await {
            Ok(session) => session,
            Err(error) => return Ok(surface_error(&error)),
        };
        self.active
            .set(item.topic, item.draft_id, Arc::clone(&session))
            .await;
        if self.store.cancellation_requested(item.update_id).await? {
            session.cancel_active_turn()?;
        }
        let draft_shutdown = CancellationToken::new();
        let events = Arc::new(SurfaceEvents::for_turn(
            Arc::clone(&self.api),
            self.store.clone(),
            item.update_id,
            item.topic,
            item.request_id,
            draft_shutdown.clone(),
        ));
        let draft_task = events.start_drafts(
            Arc::clone(&self.api),
            item.topic,
            item.draft_id,
            draft_shutdown.clone(),
        );
        let sink: Arc<dyn AgentEventSink> = events;
        let outcome = match prompt {
            Some(text) => {
                let observation = TurnObservation::from_unix_milliseconds(item.observed_at_ms)
                    .map_err(renoa_local::LocalHostError::from)?;
                session
                    .execute_turn_observed(
                        item.request_id,
                        vec![ContentBlock::text(text)],
                        observation,
                        sink,
                    )
                    .await
            }
            None => session.execute_compaction(item.request_id, sink).await,
        };
        self.active.clear(item.draft_id).await;
        draft_shutdown.cancel();
        if let Err(error) = draft_task.await {
            log::event(
                "error",
                "draft_task_failed",
                &serde_json::json!({"draft_id": item.draft_id, "error": error.to_string()}),
            );
        }
        Ok(outcome.map_or_else(|error| surface_error(&error), format_outcome))
    }

    async fn session(
        &mut self,
        session_id: Uuid,
    ) -> Result<Arc<AgentSession>, renoa_local::LocalHostError> {
        if let Some(session) = self.sessions.get(&session_id) {
            return Ok(Arc::clone(session));
        }
        let session = self
            .host
            .ensure_session(&self.profile_id, &self.workspace, session_id)
            .await?;
        self.sessions.insert(session_id, Arc::clone(&session));
        Ok(session)
    }
}

#[derive(Default)]
struct ActiveTurn {
    current: Mutex<Option<Active>>,
}

struct Active {
    topic: Topic,
    draft_id: i64,
    session: Arc<AgentSession>,
}

impl ActiveTurn {
    async fn set(&self, topic: Topic, draft_id: i64, session: Arc<AgentSession>) {
        *self.current.lock().await = Some(Active {
            topic,
            draft_id,
            session,
        });
    }

    async fn clear(&self, draft_id: i64) {
        let mut current = self.current.lock().await;
        if current
            .as_ref()
            .is_some_and(|active| active.draft_id == draft_id)
        {
            *current = None;
        }
    }

    async fn cancel(
        &self,
        topic: Topic,
        draft_id: Option<i64>,
    ) -> Result<bool, renoa_local::LocalHostError> {
        let current = self.current.lock().await;
        let Some(active) = current.as_ref().filter(|active| {
            active.topic == topic && draft_id.is_none_or(|draft| active.draft_id == draft)
        }) else {
            return Ok(false);
        };
        active.session.cancel_active_turn()?;
        Ok(true)
    }

    async fn cancel_any(&self) {
        if let Some(active) = self.current.lock().await.as_ref()
            && let Err(error) = active.session.cancel_active_turn()
        {
            log::event(
                "error",
                "shutdown_cancel_failed",
                &serde_json::json!({"error": error.to_string()}),
            );
        }
    }
}

fn format_outcome(outcome: LocalTurnOutcome) -> String {
    match outcome {
        LocalTurnOutcome::Completed { output, .. } => output,
        LocalTurnOutcome::Compacted {
            estimated_input_tokens,
        } => format!("Context compacted. Estimated context: {estimated_input_tokens} tokens."),
        LocalTurnOutcome::Cancelled => "Stopped.".to_owned(),
        LocalTurnOutcome::Failed { reason } => {
            format!("Arcee could not complete this turn: {reason}")
        }
        LocalTurnOutcome::WaitingForInput => "Arcee is waiting for more input.".to_owned(),
        _ => "Arcee returned an outcome this Telegram surface does not support yet.".to_owned(),
    }
}

fn session_status(session: &AgentSession) -> Result<String, renoa_local::LocalHostError> {
    let configuration = session.configuration()?;
    let used = session.latest_context_tokens()?;
    let window = session.context_window_tokens()?.get();
    let context = used.map_or_else(
        || "Context: no provider usage yet".to_owned(),
        |used| {
            let percent = u128::from(used)
                .saturating_mul(100)
                .checked_div(u128::from(window))
                .unwrap_or_default();
            format!("Context: {used} / {window} tokens ({percent}%)")
        },
    );
    Ok(format!(
        "Arcee session: {}\nModel: {}\nReasoning: {}\n{context}",
        session.id(),
        configuration.model,
        configuration.reasoning.name(),
    ))
}

fn surface_error(error: &renoa_local::LocalHostError) -> String {
    format!("Arcee could not complete this turn: {error}")
}

fn retry_delay(error: &ApiError, failures: u32) -> Duration {
    error.retry_after().map_or_else(
        || Duration::from_secs(1_u64 << failures.saturating_sub(1).min(5)),
        |seconds| Duration::from_secs(seconds.clamp(1, 60)),
    )
}

#[cfg(test)]
mod tests;
