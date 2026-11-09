//! The one chat the app keeps with each agent, and the lines it shows.

use md5::{Digest as _, Md5};
use renoa_protocol::{CommandInput, ExecutionEventKind, ExecutionTerminal};
use renoa_rcp_wire::{NodeId, TaskEvent, TaskEventKind, TaskId};
use uuid::Builder;

/// The chat task every app of the owner opens with the agent `target` on
/// `node_id`: MD5 of the text below with the version 3 bits set, as Java's
/// `UUID.nameUUIDFromBytes` makes it, so the phone and the Mac share one
/// conversation.
pub fn chat_task_id(node_id: NodeId, target: &str) -> TaskId {
    let text = format!("renoa app chat v1\n{}\n{target}", node_id.as_uuid());
    let digest: [u8; 16] = Md5::digest(text.as_bytes()).into();
    TaskId::from_uuid(Builder::from_md5_bytes(digest).into_uuid())
}

/// Who said a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Owner,
    Agent,
}

/// One line of a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub speaker: Speaker,
    pub text: String,
    /// Whether the line says a turn failed rather than what the agent said.
    pub failed: bool,
    /// When it was said, in Unix milliseconds. An owner's line has no time of
    /// its own: it takes the time the agent began answering it.
    pub at_ms: Option<i64>,
}

/// A chat as its task's events tell it, in order, each event once.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<Line>,
    through: Option<u64>,
    working: bool,
}

impl Transcript {
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The last sequence applied, to attach after when reconnecting.
    pub const fn through(&self) -> Option<u64> {
        self.through
    }

    /// Whether the agent is answering.
    pub const fn working(&self) -> bool {
        self.working
    }

    /// Applies `event` unless an event at or past its sequence already was:
    /// a replay after a reconnect repeats events.
    pub fn apply(&mut self, event: &TaskEvent) {
        if self
            .through
            .is_some_and(|through| event.sequence <= through)
        {
            return;
        }
        self.through = Some(event.sequence);
        match &event.kind {
            TaskEventKind::CommandSubmitted { command } => {
                let CommandInput::Text {
                    text,
                    quiet,
                    author,
                    ..
                } = &command.input;
                // Another agent's message and a quiet command are not the owner speaking.
                if !quiet && author.is_principal() && command.origin.is_none() {
                    self.say(Speaker::Owner, text, false, None);
                }
            }
            TaskEventKind::ExecutionEvent { event, .. } => {
                let at_ms = Some(event.recorded_at_ms);
                for line in self.lines.iter_mut().rev() {
                    if line.at_ms.is_some() {
                        break;
                    }
                    line.at_ms = at_ms;
                }
                match &event.kind {
                    ExecutionEventKind::TurnStarted | ExecutionEventKind::ExecutionStarted => {
                        self.working = true;
                    }
                    ExecutionEventKind::AssistantMessage { text } => {
                        self.say(Speaker::Agent, text, false, at_ms);
                    }
                    ExecutionEventKind::ExecutionTerminated { terminal } => {
                        self.working = false;
                        if let ExecutionTerminal::Failed { .. } = terminal {
                            let text = "Something went wrong with that one.";
                            self.say(Speaker::Agent, text, true, at_ms);
                        }
                    }
                    ExecutionEventKind::ToolStarted { .. }
                    | ExecutionEventKind::ToolFinished { .. } => {}
                }
            }
        }
    }

    fn say(&mut self, speaker: Speaker, text: &str, failed: bool, at_ms: Option<i64>) {
        let text = text.trim();
        if !text.is_empty() {
            self.lines.push(Line {
                speaker,
                text: text.to_owned(),
                failed,
                at_ms,
            });
        }
    }
}

#[cfg(test)]
mod tests;
