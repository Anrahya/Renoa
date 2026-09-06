use std::sync::Arc;

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_local::{AgentSession, TurnObservation};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{Worker, format_outcome, surface_error};
use crate::{TelegramServiceError, events::SurfaceEvents, log, store::WorkItem};

impl Worker {
    pub(super) async fn run_agent(
        &mut self,
        item: &WorkItem,
        prompt: Option<&str>,
    ) -> Result<String, TelegramServiceError> {
        let cancellation = CancellationToken::new();
        self.active
            .set(item.topic, item.draft_id, cancellation.clone())
            .await;
        let result = self.run_registered_agent(item, prompt, cancellation).await;
        self.active.clear(item.draft_id).await;
        result
    }

    async fn run_registered_agent(
        &mut self,
        item: &WorkItem,
        prompt: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<String, TelegramServiceError> {
        if self.store.cancellation_requested(item.update_id).await? {
            cancellation.cancel();
        }
        if cancellation.is_cancelled()
            && let Some(result) = self.cancelled_result(item, prompt).await
        {
            return Ok(result);
        }
        let session = match self.session(item.session_id).await {
            Ok(session) => session,
            Err(error) => {
                if cancellation.is_cancelled()
                    && let Some(result) = self.cancelled_result(item, prompt).await
                {
                    return Ok(result);
                }
                return Ok(surface_error(&error));
            }
        };
        #[cfg(test)]
        if let Some((started, release)) = self.before_execution.take() {
            started.send(()).expect("startup boundary receiver");
            release.await.expect("release startup boundary");
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
                    .execute_turn_observed_with_cancellation(
                        item.request_id,
                        vec![ContentBlock::text(text)],
                        observation,
                        sink,
                        cancellation,
                    )
                    .await
            }
            None => {
                session
                    .execute_compaction_with_cancellation(item.request_id, sink, cancellation)
                    .await
            }
        };
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

    async fn cancelled_result(&self, item: &WorkItem, prompt: Option<&str>) -> Option<String> {
        let content = prompt.map(|text| vec![ContentBlock::text(text)]);
        let result = if let Some(session) = self.sessions.get(&item.session_id) {
            session.cancel_before_execution(item.request_id, content.as_deref())
        } else {
            self.host
                .cancel_before_execution(
                    &self.profile_id,
                    &self.workspace,
                    item.session_id,
                    item.request_id,
                    content.as_deref(),
                )
                .await
        };
        match result {
            Ok(outcome) => outcome.map(format_outcome),
            Err(error) => Some(surface_error(&error)),
        }
    }

    pub(super) async fn session(
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
