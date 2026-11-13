//! The one chat the app keeps with each agent, and the lines it shows.

use std::{
    collections::HashSet,
    ops::Range,
    time::{Duration, Instant},
};

use md5::{Digest as _, Md5};
use renoa_protocol::{
    CommandId, CommandInput, ExecutionEventKind, ExecutionTerminal, LiveActivity,
    normalize_activity,
};
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

/// How long after it was said a line still counts as new: a replay of older
/// lines, on first opening a chat, comes in still.
const FRESH_FOR_MS: i64 = 30_000;

/// How long a new line takes to come in.
const COMING_IN: Duration = Duration::from_millis(600);

/// A chat as its task's events tell it, in order, each event once.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<Line>,
    /// When each line arrived, if it was new then.
    arrived: Vec<Option<Instant>>,
    through: Option<u64>,
    working: bool,
    doing: Option<String>,
    latest_command: Option<CommandId>,
    terminated: HashSet<CommandId>,
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

    /// Plain-text one-line summary of what the agent is currently doing, if any.
    pub fn doing(&self) -> Option<&str> {
        self.doing.as_deref()
    }

    /// Whether the line at `index` is still coming in: it was new when it
    /// arrived, a moment ago. Decided once, so a line scrolled away and back
    /// does not come in again.
    pub fn coming_in(&self, index: usize) -> bool {
        self.arrived
            .get(index)
            .copied()
            .flatten()
            .is_some_and(|arrived| arrived.elapsed() < COMING_IN)
    }

    /// Applies `event`, arriving at `now_ms` by this Mac's clock, unless an
    /// event at or past its sequence already was: a replay after a reconnect
    /// repeats events. Returns the lines that took its time, which may now
    /// show a time above them.
    pub fn apply(&mut self, event: &TaskEvent, now_ms: i64) -> Option<Range<usize>> {
        if self
            .through
            .is_some_and(|through| event.sequence <= through)
        {
            return None;
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
                self.latest_command = Some(command.command_id);
                self.doing = None;
                // Another agent's message and a quiet command are not the owner speaking.
                if !quiet && author.is_principal() && command.origin.is_none() {
                    // New until its time says otherwise.
                    self.say(Speaker::Owner, text, false, None, true);
                }
                None
            }
            TaskEventKind::ExecutionEvent { command_id, event } => {
                let at_ms = Some(event.recorded_at_ms);
                let fresh = now_ms - event.recorded_at_ms < FRESH_FOR_MS;
                let untimed = self
                    .lines
                    .iter()
                    .rev()
                    .take_while(|line| line.at_ms.is_none())
                    .count();
                let took = self.lines.len() - untimed..self.lines.len();
                for (line, arrived) in self.lines[took.clone()]
                    .iter_mut()
                    .zip(&mut self.arrived[took.clone()])
                {
                    line.at_ms = at_ms;
                    if !fresh {
                        *arrived = None;
                    }
                }
                match &event.kind {
                    ExecutionEventKind::TurnStarted | ExecutionEventKind::ExecutionStarted => {
                        self.working = true;
                        self.latest_command = Some(*command_id);
                    }
                    ExecutionEventKind::AssistantMessage { text } => {
                        self.doing = None;
                        self.say(Speaker::Agent, text, false, at_ms, fresh);
                    }
                    ExecutionEventKind::ExecutionTerminated { terminal } => {
                        self.terminated.insert(*command_id);
                        self.working = false;
                        self.doing = None;
                        if let ExecutionTerminal::Failed { .. } = terminal {
                            let text = "Something went wrong with that one.";
                            self.say(Speaker::Agent, text, true, at_ms, fresh);
                        }
                    }
                    ExecutionEventKind::ToolStarted { activity, .. } => {
                        self.working = true;
                        self.doing = activity.as_deref().and_then(normalize_activity);
                    }
                    ExecutionEventKind::ToolFinished { .. } => {}
                }
                (!took.is_empty()).then_some(took)
            }
        }
    }

    /// Applies one ephemeral live activity frame, returning whether `doing` changed.
    pub fn apply_live(&mut self, command_id: CommandId, activity: &LiveActivity) -> bool {
        if self.terminated.contains(&command_id)
            || self
                .latest_command
                .is_some_and(|latest| latest != command_id)
        {
            return false;
        }
        match activity {
            LiveActivity::Activity { text } => {
                let next = normalize_activity(text);
                if next == self.doing {
                    return false;
                }
                self.doing = next;
                true
            }
            LiveActivity::TextReset => self.doing.take().is_some(),
            LiveActivity::TextDelta { .. } => false,
        }
    }

    /// Clears transient live activity after the coordinator link drops.
    pub fn clear_live(&mut self) {
        self.doing = None;
    }

    fn say(&mut self, speaker: Speaker, text: &str, failed: bool, at_ms: Option<i64>, fresh: bool) {
        let text = text.trim();
        if !text.is_empty() {
            self.lines.push(Line {
                speaker,
                text: text.to_owned(),
                failed,
                at_ms,
            });
            self.arrived.push(fresh.then(Instant::now));
        }
    }
}

#[cfg(test)]
mod tests;
