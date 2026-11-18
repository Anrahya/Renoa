//! Steering: a message that joins the turn already running on its task.
//!
//! The coordinator delivers a steer like any command: the node admits it to its
//! ledger and queues it behind the running command. While that command's turn
//! runs, the node also offers the steer to it ([`AgentSession::steer`]); the
//! turn takes it in before its next model call. When the steer's own place in
//! the queue comes, the node ends it as completed if the turn took it, and
//! otherwise runs it as an ordinary turn. Offers are not recorded by the node:
//! a restarted node offers again, and the Host converges on the first record.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use renoa_local::{AgentSession, Speaker, TurnObservation};
use renoa_protocol::{CommandEnvelope, CommandId};
use renoa_rcp_wire::TaskId;
use tokio::sync::mpsc;

use crate::{
    bridge::NodeRuntime,
    control::{self, Request},
    node_log,
    node_store::ExecutionRecord,
    origin,
};

/// The command running on each task, and the steers already sent to its turn.
#[derive(Default)]
pub(crate) struct RunningTasks(HashMap<TaskId, RunningCommand>);

struct RunningCommand {
    command: CommandEnvelope,
    /// Unbounded: each steer is a command the ledger already admitted, and
    /// each is sent at most once per run.
    steers: mpsc::UnboundedSender<ExecutionRecord>,
    sent: HashSet<CommandId>,
}

/// The steers a running turn receives while it runs.
pub(crate) type SteerInbox = mpsc::UnboundedReceiver<ExecutionRecord>;

impl RunningTasks {
    /// Marks `record` running on its task and returns the inbox of its turn.
    pub(crate) fn start(&mut self, record: &ExecutionRecord) -> SteerInbox {
        let (steers, inbox) = mpsc::unbounded_channel();
        self.0.insert(
            record.task_id,
            RunningCommand {
                command: record.command.clone(),
                steers,
                sent: HashSet::new(),
            },
        );
        inbox
    }

    /// Whether a command runs on the task of `queued`, which is then queued
    /// behind it. If `queued` may steer that command and no earlier queued
    /// message of the task waits (`held` names the tasks where one does), it
    /// is sent to the running turn once. A steer never overtakes an earlier
    /// message: that message is answered first, in queue order. Records come
    /// in admission order, the running command first.
    pub(crate) fn queue_behind(
        &mut self,
        queued: &ExecutionRecord,
        held: &mut HashSet<TaskId>,
    ) -> bool {
        let Some(running) = self.0.get_mut(&queued.task_id) else {
            return false;
        };
        if queued.command.command_id == running.command.command_id {
            return true;
        }
        if held.contains(&queued.task_id) || !may_steer(&running.command, &queued.command) {
            held.insert(queued.task_id);
        } else if running.sent.insert(queued.command.command_id) {
            // A closed inbox means the turn has ended: the steer runs on its own.
            let _ended = running.steers.send(queued.clone());
        }
        true
    }

    pub(crate) fn finish(&mut self, task_id: TaskId) {
        self.0.remove(&task_id);
    }
}

/// Whether `steer` may join the turn of `running`: the person who sent it
/// asked for that, both are messages from that same person, and neither is a
/// control such as `/compact`. A guest's or another agent's message, and a
/// message for a turn someone else started, wait for their own turn.
fn may_steer(running: &CommandEnvelope, steer: &CommandEnvelope) -> bool {
    let person = |command: &CommandEnvelope| match origin::speaker(command) {
        Speaker::Principal(person) => Some(person),
        Speaker::Unidentified | Speaker::Guest => None,
    };
    steer.input.steer()
        && person(steer).is_some()
        && person(steer) == person(running)
        && control::request(running) == Ok(Request::Turn)
        && control::request(steer) == Ok(Request::Turn)
}

impl NodeRuntime {
    /// Offers each steer sent to the turn of `running` until the inbox closes.
    /// The caller drops this future when the turn ends; an offer cut short
    /// leaves the steer recorded or not, and either way it is settled by the
    /// steer's own place in the queue.
    pub(crate) async fn offer_steers(
        &self,
        running: CommandId,
        session: &AgentSession,
        workspace: &Path,
        mut inbox: SteerInbox,
    ) {
        while let Some(steer) = inbox.recv().await {
            let command = &steer.command;
            // A steer stopped before it is offered never joins. A stop that
            // comes after the turn took it in is too late: its message is part
            // of that turn, and it ends completed.
            let offered = match self.state.stop_requested(command.command_id).await {
                Ok(true) => Ok("stopped".to_owned()),
                Ok(false) => self.offer(running, session, workspace, command).await,
                Err(error) => Err(error.to_string()),
            };
            let (level, fields) = match offered {
                Ok(state) => ("info", serde_json::json!({ "state": state })),
                Err(error) => ("warn", serde_json::json!({ "error": error })),
            };
            node_log::event(
                level,
                "steer_offered",
                &serde_json::json!({
                    "task_id": steer.task_id,
                    "command_id": command.command_id,
                    "running": running,
                    "outcome": fields,
                }),
            );
        }
    }

    /// Offers `command` to the turn of `running`; the state it is recorded in.
    async fn offer(
        &self,
        running: CommandId,
        session: &AgentSession,
        workspace: &Path,
        command: &CommandEnvelope,
    ) -> Result<String, String> {
        match crate::execution::turn_content(&self.host, command, workspace).await {
            Ok(content) => match TurnObservation::now() {
                Ok(observation) => session
                    .steer(
                        running.as_uuid(),
                        command.command_id.as_uuid(),
                        content,
                        origin::observed(command, observation),
                        origin::speaker(command),
                    )
                    .await
                    .map(|state| format!("{state:?}").to_lowercase())
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use renoa_protocol::{
        AgentMessageKind, AgentOrigin, Author, CommandEnvelope, CommandId, CommandInput,
        PrincipalId, SurfaceRef, TargetRef,
    };

    use std::collections::HashSet;

    use renoa_protocol::ExecutionId;
    use renoa_rcp_wire::TaskId;

    use super::{RunningTasks, may_steer};
    use crate::node_store::{ExecutionRecord, TargetBinding};

    fn record(task_id: TaskId, command: CommandEnvelope) -> ExecutionRecord {
        ExecutionRecord {
            admission_sequence: 0,
            task_id,
            execution_id: ExecutionId::from_uuid(command.command_id.as_uuid()),
            command,
            binding: TargetBinding {
                target: "agent:test".to_owned(),
                agent_id: uuid::Uuid::nil(),
                session_id: uuid::Uuid::nil(),
                workspace: std::path::PathBuf::new(),
            },
            admission_acked: true,
            terminal: false,
            published_through: None,
        }
    }

    #[test]
    fn a_steer_joins_only_ahead_of_no_earlier_message_and_never_its_own_turn() {
        let owner = PrincipalId::new();
        let task = TaskId::new();
        let running = record(
            task,
            command(owner, "Make a video.", Author::Principal, true),
        );
        let first = record(task, command(owner, "Shorter.", Author::Principal, true));
        let queued = record(
            task,
            command(owner, "Then a poster.", Author::Principal, false),
        );
        let later = record(task, command(owner, "Blue.", Author::Principal, true));
        let mut tasks = RunningTasks::default();
        let mut inbox = tasks.start(&running);
        let mut held = HashSet::new();

        for queued in [&running, &first, &queued, &later, &first] {
            assert!(tasks.queue_behind(queued, &mut held));
        }

        let sent = inbox.try_recv().expect("the first steer is sent");
        assert_eq!(sent.command.command_id, first.command.command_id);
        assert!(
            inbox.try_recv().is_err(),
            "the running command, a repeat and a steer behind a queued message are not sent"
        );
        assert!(!tasks.queue_behind(&record(TaskId::new(), later.command), &mut held));
    }

    fn command(principal: PrincipalId, text: &str, author: Author, steer: bool) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(),
            principal_id: principal,
            surface: SurfaceRef::new("test"),
            target: TargetRef::new("agent:test"),
            input: CommandInput::Text {
                text: text.to_owned(),
                context: None,
                author,
                quiet: false,
                steer,
                attachments: Vec::new(),
            },
            origin: None,
        }
    }

    #[test]
    fn only_the_person_running_a_turn_steers_it_with_a_message() {
        let owner = PrincipalId::new();
        let running = command(owner, "Make a video.", Author::Principal, false);
        assert!(may_steer(
            &running,
            &command(owner, "Shorter.", Author::Principal, true)
        ));

        assert!(
            !may_steer(
                &running,
                &command(owner, "Shorter.", Author::Principal, false)
            ),
            "a message sent to queue waits"
        );
        assert!(
            !may_steer(&running, &command(owner, "Shorter.", Author::Guest, true)),
            "a guest cannot join the owner's turn"
        );
        assert!(
            !may_steer(
                &running,
                &command(PrincipalId::new(), "Shorter.", Author::Principal, true)
            ),
            "another person cannot join it"
        );
        assert!(
            !may_steer(
                &running,
                &command(owner, "/compact", Author::Principal, true)
            ),
            "a control is not a message"
        );
        assert!(
            !may_steer(
                &command(owner, "/compact", Author::Principal, false),
                &command(owner, "Shorter.", Author::Principal, true)
            ),
            "a compaction has no model call to join"
        );
        let mut from_agent = command(owner, "Please check.", Author::Principal, false);
        from_agent.origin = Some(AgentOrigin {
            from_command: CommandId::new(),
            from: TargetRef::new("agent:other"),
            name: "Other".to_owned(),
            depth: 1,
            kind: AgentMessageKind::Request,
        });
        assert!(
            !may_steer(
                &from_agent,
                &command(owner, "Shorter.", Author::Principal, true)
            ),
            "a turn another agent started is not the owner's to steer"
        );
    }
}
