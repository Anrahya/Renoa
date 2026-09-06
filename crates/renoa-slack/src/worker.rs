use std::{path::PathBuf, sync::Arc, time::Duration};

use renoa_agent::ContentBlock;
use renoa_kernel::AgentId;
use renoa_local::{AgentSession, LocalHost, LocalTurnOutcome, TurnObservation};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{
    SlackError,
    api::{ApiError, SlackApi},
    commands::Command,
    events::Progress,
    service::Active,
    store::{Delivery, DeliveryState, ReplyState, Store, Work},
};

pub(crate) struct Worker {
    pub(crate) host: LocalHost,
    pub(crate) agent_id: AgentId,
    pub(crate) workspace: PathBuf,
    pub(crate) api: Arc<SlackApi>,
    pub(crate) store: Store,
    pub(crate) active: Arc<Active>,
    pub(crate) wake: Arc<Notify>,
    pub(crate) channel_wake: Arc<Notify>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) session: Option<Arc<AgentSession>>,
}

impl Worker {
    pub(crate) async fn run(mut self) -> Result<(), SlackError> {
        loop {
            if self.shutdown.is_cancelled() {
                return Ok(());
            }
            if let Some(delivery) = self.store.next_delivery().await? {
                self.deliver(delivery).await?;
            } else if let Some(work) = self.store.next_work().await? {
                self.execute(work).await?;
            } else {
                tokio::select! { () = self.shutdown.cancelled() => return Ok(()), () = self.wake.notified() => {} }
            }
        }
    }

    pub(crate) async fn execute(&mut self, mut work: Work) -> Result<(), SlackError> {
        self.store.mark_running(work.seq).await?;
        let cancellation = self.shutdown.child_token();
        self.active
            .register(work.request_id, cancellation.clone())
            .await;
        let result = self.execute_registered(&mut work, cancellation).await;
        self.active.clear().await;
        let text = result?;
        self.store.finish(work.seq, text).await?;
        self.channel_wake.notify_one();
        Ok(())
    }

    async fn execute_registered(
        &mut self,
        work: &mut Work,
        cancellation: CancellationToken,
    ) -> Result<String, SlackError> {
        if self.store.cancelled(work.request_id).await? {
            cancellation.cancel();
        }
        if work.command.executes_model()
            && cancellation.is_cancelled()
            && let Some(result) = self.cancel_before_start(work).await?
        {
            return Ok(result);
        }
        if work.reply_pending {
            self.start_reply(work, &cancellation).await?;
        }
        if work.command.executes_model()
            && cancellation.is_cancelled()
            && let Some(result) = self.cancel_before_start(work).await?
        {
            return Ok(result);
        }
        match &work.command {
            Command::Notice(text) => return Ok(text.clone()),
            Command::Agent(Some(_)) => return Ok("Started a fresh conversation with the selected agent. Send its task here. Use !agent arcee to return to Arcee.".to_owned()),
            Command::Agent(None) => {
                let page=self.host.list_bots(None).await?;
                let mut lines = vec!["Each specialist gets a private channel. Type there normally; !new resets its conversation. Channel setup status:".to_owned()];
                for bot in page.bots {
                    lines.push(format!("{} — {} — {}", bot.name, bot.id, self.store.channel_description(bot.id.to_string()).await?));
                }
                if page.next_cursor.is_some() { lines.push("More bots exist; ask Arcee to page through bot_manage.".to_owned()); }
                return Ok(lines.join("\n"));
            }
            Command::Cancel => return Ok(if work.cancel_target.is_some() {"Stop requested."} else {"There is no pending turn to stop in this conversation."}.to_owned()),
            Command::New => return Ok("Started a fresh conversation here.".to_owned()),
            Command::Help => return Ok("Send a task here, or use !agent, !new, !status, !model [id], !reasoning [level], !compact, or !cancel. In channels, mention Arcee to start a thread and continue inside that thread.".to_owned()),
            _ => {},
        }
        let session = match self.session(work.session_id).await {
            Ok(session) => session,
            Err(error) if cancellation.is_cancelled() && work.command.executes_model() => {
                eprintln!("Slack execution startup failed during cancellation: {error}");
                if let Some(result) = self.cancel_before_start(work).await? {
                    return Ok(result);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if !work.command.executes_model() {
            return Ok(super::controls::run(&session, &work.command)
                .await
                .unwrap_or_else(|error| format!("Could not apply that setting: {error}")));
        }
        let progress_stop = CancellationToken::new();
        let (sink, task) = if let Some(ts) = &work.reply_ts {
            let (sink, task) = Progress::start(
                Arc::clone(&self.api),
                work.topic.clone(),
                ts.clone(),
                progress_stop.clone(),
            );
            (sink, Some(task))
        } else {
            (Progress::quiet(), None)
        };
        let outcome = async {
            match &work.command {
                Command::Prompt(text) => {
                    let observation = TurnObservation::from_unix_milliseconds(work.observed_at_ms)
                        .map_err(renoa_local::LocalHostError::from)?;
                    session
                        .execute_turn_observed_with_cancellation(
                            work.request_id,
                            vec![ContentBlock::text(text)],
                            observation,
                            sink,
                            cancellation,
                        )
                        .await
                }
                Command::Compact => {
                    session
                        .execute_compaction_with_cancellation(work.request_id, sink, cancellation)
                        .await
                }
                _ => Err(renoa_local::LocalHostError::InvalidRequest(
                    "non-model command reached execution".to_owned(),
                )),
            }
        }
        .await;
        progress_stop.cancel();
        if let Some(task) = task {
            task.await?;
        }
        Ok(format_outcome(outcome?))
    }

    async fn session(&mut self, id: uuid::Uuid) -> Result<Arc<AgentSession>, SlackError> {
        if let Some(session) = &self.session
            && session.id() == id
        {
            return Ok(Arc::clone(session));
        }
        // One operator worker owns at most one live kernel. Durable sessions
        // outlive this bounded cache and reopen on conversation switches.
        self.session = None;
        let agent_id = AgentId::from_uuid(self.store.session_agent(id).await?);
        let workspace = if agent_id == self.agent_id {
            self.workspace.clone()
        } else {
            self.host.bot_workspace(agent_id).await?
        };
        let session = self
            .host
            .ensure_agent_session(agent_id, &workspace, id)
            .await?;
        self.session = Some(Arc::clone(&session));
        Ok(session)
    }

    async fn cancel_before_start(&self, work: &Work) -> Result<Option<String>, SlackError> {
        let content = match &work.command {
            Command::Prompt(text) => Some(vec![ContentBlock::text(text)]),
            _ => None,
        };
        let result = if let Some(session) = &self.session
            && session.id() == work.session_id
        {
            session.cancel_before_execution(work.request_id, content.as_deref())?
        } else {
            let agent_id = AgentId::from_uuid(self.store.session_agent(work.session_id).await?);
            let agent = self
                .host
                .agent(agent_id)
                .await?
                .ok_or(renoa_local::LocalHostError::AgentNotFound(agent_id))?;
            let workspace = if agent_id == self.agent_id {
                self.workspace.clone()
            } else {
                self.host.bot_workspace(agent_id).await?
            };
            self.host
                .cancel_before_execution(
                    &agent.profile,
                    &workspace,
                    work.session_id,
                    work.request_id,
                    content.as_deref(),
                )
                .await?
        };
        Ok(result.map(format_outcome))
    }

    async fn start_reply(
        &self,
        work: &mut Work,
        cancellation: &CancellationToken,
    ) -> Result<(), SlackError> {
        loop {
            if cancellation.is_cancelled() {
                return Ok(());
            }
            self.store
                .reply_state(work.seq, ReplyState::Sending, None)
                .await?;
            match self
                .api
                .post(
                    &work.topic,
                    "Request received. Arcee will update this conversation with the result.",
                )
                .await
            {
                Ok(sent) => {
                    self.store
                        .reply_state(work.seq, ReplyState::Known, Some(sent.ts.clone()))
                        .await?;
                    work.reply_ts = Some(sent.ts);
                    return Ok(());
                }
                Err(ApiError::RateLimited(delay)) => {
                    self.store
                        .reply_state(work.seq, ReplyState::Pending, None)
                        .await?;
                    tokio::select! { () = cancellation.cancelled() => return Ok(()), () = tokio::time::sleep(delay) => {} }
                    if self.shutdown.is_cancelled() {
                        return Err(SlackError::Invalid("surface is shutting down".to_owned()));
                    }
                }
                Err(error) => {
                    let state = if matches!(error, ApiError::Rejected(_)) {
                        ReplyState::Failed
                    } else {
                        ReplyState::Unknown
                    };
                    self.store.reply_state(work.seq, state, None).await?;
                    eprintln!("Slack acknowledgement delivery {}: {error}", state.as_str());
                    return Ok(());
                }
            }
        }
    }

    pub(crate) async fn deliver(&self, delivery: Delivery) -> Result<(), SlackError> {
        self.store
            .delivery_state(
                delivery.seq,
                delivery.chunk,
                DeliveryState::Sending,
                None,
                None,
            )
            .await?;
        let result = if let Some(ts) = &delivery.ts {
            self.api
                .update(&delivery.topic, ts, &delivery.text)
                .await
                .map(|()| ts.clone())
        } else {
            self.api
                .post(&delivery.topic, &delivery.text)
                .await
                .map(|sent| sent.ts)
        };
        let (state, ts, error, delay) = match result {
            Ok(ts) => (DeliveryState::Sent, Some(ts), None, Duration::from_secs(1)),
            Err(ApiError::RateLimited(delay)) => (DeliveryState::Pending, None, None, delay),
            Err(error) => {
                let state = if matches!(error, ApiError::Rejected(_)) {
                    DeliveryState::Failed
                } else if delivery.ts.is_some() {
                    DeliveryState::Pending
                } else {
                    DeliveryState::Unknown
                };
                eprintln!(
                    "Slack result delivery {}, request {}, chunk {}: {error}",
                    state.as_str(),
                    delivery.seq,
                    delivery.chunk
                );
                (state, None, Some(error.to_string()), Duration::from_secs(5))
            }
        };
        self.store
            .delivery_state(delivery.seq, delivery.chunk, state, ts, error)
            .await?;
        self.pause(delay).await;
        Ok(())
    }

    async fn pause(&self, delay: Duration) {
        tokio::select! { () = self.shutdown.cancelled() => {}, () = tokio::time::sleep(delay) => {} }
    }
}

fn format_outcome(outcome: LocalTurnOutcome) -> String {
    match outcome {
        LocalTurnOutcome::Completed { output, .. } => output,
        LocalTurnOutcome::Cancelled => "Stopped.".to_owned(),
        LocalTurnOutcome::Compacted {
            estimated_input_tokens,
        } => format!("Context compacted to approximately {estimated_input_tokens} tokens."),
        LocalTurnOutcome::Failed { reason } => {
            format!("Arcee could not complete this turn: {reason}")
        }
        LocalTurnOutcome::WaitingForInput => "Arcee is waiting for more input.".to_owned(),
        _ => "Arcee returned an unsupported outcome; inspect the durable session.".to_owned(),
    }
}
