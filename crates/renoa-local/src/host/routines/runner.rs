use super::{LocalHost, LocalHostError, RoutineRun, store};
use crate::{LocalTurnOutcome, TurnObservation};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use std::{fs::OpenOptions, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

struct HeadlessProgress(CancellationToken);
impl AgentEventSink for HeadlessProgress {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        if let AgentEvent::ToolExecutionUpdate { call, update } = event
            && call.name == "extension_manage"
            && !update.is_error
            && let [ContentBlock::Text { text }] = update.content.as_slice()
        {
            #[derive(serde::Deserialize)]
            struct Setup {
                status: String,
            }
            if let Ok(setup) = serde_json::from_str::<Setup>(text)
                && matches!(
                    setup.status.as_str(),
                    "credential_required" | "authorization_required"
                )
            {
                // No interactive surface owns this run. Stop the wait so one
                // unattended OAuth flow cannot block the Host's entire queue.
                self.0.cancel();
            }
        }
        Box::pin(async {})
    }
}

impl LocalHost {
    /// Runs the Host-owned routine scheduler until shutdown. One process owns
    /// admission/execution; shutdown drains the active turn before returning.
    /// # Errors
    /// Returns ownership, storage, and execution infrastructure failures. Pending
    /// commands remain durable and are replayed by the next service instance.
    pub async fn run_routines(&self, shutdown: CancellationToken) -> Result<(), LocalHostError> {
        let lock = self.config.database.with_file_name(".routines.lock");
        let lease = tokio::task::spawn_blocking(move || {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock)?;
            file.try_lock()?;
            Ok::<_, std::io::Error>(file)
        })
        .await??;
        while !shutdown.is_cancelled() {
            let now = TurnObservation::now()?.unix_milliseconds();
            let database = self.config.database.clone();
            let next = tokio::task::spawn_blocking(move || store::next(&database, now)).await??;
            if let Some(run) = next {
                self.execute_routine_run(run).await?;
            } else {
                tokio::select! {()=shutdown.cancelled()=>{},()=tokio::time::sleep(Duration::from_secs(1))=>{}}
            }
        }
        drop(lease);
        Ok(())
    }

    pub(super) async fn execute_routine_run(&self, run: RoutineRun) -> Result<(), LocalHostError> {
        let workspace = self.bot_workspace(run.agent_id).await?;
        let session = self
            .ensure_agent_session(run.agent_id, &workspace, run.session_id)
            .await?;
        let cancellation = CancellationToken::new();
        let outcome = session
            .execute_turn_observed_with_cancellation(
                run.id,
                vec![ContentBlock::text(&run.prompt)],
                TurnObservation::from_unix_milliseconds(run.admitted_at_ms)?,
                Arc::new(HeadlessProgress(cancellation.clone())),
                cancellation,
            )
            .await?;
        let output = match outcome {
            LocalTurnOutcome::Completed { output, .. } => output,
            LocalTurnOutcome::Cancelled => "Scheduled run stopped. If account setup is needed, resolve it interactively with this specialist before running again.".to_owned(),
            LocalTurnOutcome::Failed { reason } => format!("Scheduled run failed: {reason}"),
            LocalTurnOutcome::WaitingForInput => {
                "Scheduled run needs input. Continue with the specialist to resolve it.".to_owned()
            }
            _ => {
                return Err(LocalHostError::InvalidRequest(
                    "unsupported scheduled turn outcome".to_owned(),
                ));
            }
        };
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || store::finish(&database, run.id, &output)).await??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unattended_account_setup_stops_the_wait_instead_of_blocking_the_scheduler() {
        for status in ["credential_required", "authorization_required"] {
            let cancellation = CancellationToken::new();
            let sink = HeadlessProgress(cancellation.clone());
            sink.emit(AgentEvent::ToolExecutionUpdate{
                call:renoa_agent::ToolCall{id:"setup".to_owned(),name:"extension_manage".to_owned(),arguments:serde_json::json!({}),namespace:None,thought_signature:None},
                update:renoa_agent::ToolOutput{content:vec![ContentBlock::text(serde_json::json!({"status":status,"authorization_url":"https://example.com/private"}).to_string())],details:None,is_error:false}
            }).await;
            assert!(cancellation.is_cancelled());
        }
    }
}
