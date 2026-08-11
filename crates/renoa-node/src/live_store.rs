use std::sync::Arc;

use renoa_control::TaskId;
use renoa_core::{
    BoxFuture, CommandEnvelope, CommandId, ResolvedAgent, RunAdmission, RunEventKind, RunId,
    RunStore, RunTranscript, StoreError, TerminalState,
};
use renoa_store_sqlite::SqliteRunStore;
use tokio::sync::watch;

use crate::node_store::NodeStore;

/// Turns durable ledger commits into coalescing wakeups for the network
/// publisher. The ledger remains the source of truth if a wakeup is skipped.
pub(crate) struct LiveRunStore {
    inner: Arc<SqliteRunStore>,
    node_store: NodeStore,
    task_id: TaskId,
    command_id: CommandId,
    commits: watch::Sender<u64>,
}

impl LiveRunStore {
    pub(crate) fn new(
        inner: Arc<SqliteRunStore>,
        node_store: NodeStore,
        task_id: TaskId,
        command_id: CommandId,
        commits: watch::Sender<u64>,
    ) -> Self {
        Self {
            inner,
            node_store,
            task_id,
            command_id,
            commits,
        }
    }

    fn signal_commit(&self) {
        self.commits.send_modify(|version| {
            *version = version.wrapping_add(1);
        });
    }
}

impl RunStore for LiveRunStore {
    fn admit_run(
        &self,
        command: CommandEnvelope,
        agent: ResolvedAgent,
    ) -> BoxFuture<'_, Result<RunAdmission, StoreError>> {
        Box::pin(async move {
            if command.command_id != self.command_id {
                return Err(StoreError::new("execution command identity changed"));
            }
            let admission = self.inner.admit_run(command, agent).await?;
            let run_id = match admission {
                RunAdmission::Admitted(run_id)
                | RunAdmission::Existing(run_id)
                | RunAdmission::Conflict(run_id) => run_id,
            };
            if !matches!(admission, RunAdmission::Conflict(_)) {
                self.node_store
                    .admit(self.task_id, self.command_id, run_id)
                    .await?;
                self.signal_commit();
            }
            Ok(admission)
        })
    }

    fn append_events(
        &self,
        run_id: RunId,
        events: Vec<RunEventKind>,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.inner.append_events(run_id, events).await?;
            self.signal_commit();
            Ok(())
        })
    }

    fn finish_run(
        &self,
        run_id: RunId,
        terminal: TerminalState,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.inner.finish_run(run_id, terminal).await?;
            self.signal_commit();
            Ok(())
        })
    }

    fn load_transcript(&self, run_id: RunId) -> BoxFuture<'_, Result<RunTranscript, StoreError>> {
        self.inner.load_transcript(run_id)
    }
}
