//! Transient progress for running commands: a typing indicator in the
//! command's channel and one progress message, edited in place, listing its
//! tool calls and the intermediate messages that led to them. Once the command
//! finishes and its answer has had time to post, the progress message is
//! deleted.
//!
//! Progress is presence, not history. Nothing here is stored: after a restart
//! progress resumes from the next task record, and a failed Discord call is
//! retried on the next tick or dropped. A command that calls no tool shows only
//! the typing indicator, and a command that finishes before its progress
//! message was posted never posts one.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

use renoa_control::TaskEventKind;
use renoa_protocol::ExecutionEventKind;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    store::ProgressTarget,
};

/// Discord shows a typing indicator for about ten seconds.
const TYPING_INTERVAL: Duration = Duration::from_secs(8);
/// Edits stay well inside Discord's per-channel message rate limit.
const EDIT_INTERVAL: Duration = Duration::from_millis(1500);
/// A command that reports nothing for this long is no longer shown as working.
const IDLE_LIMIT: Duration = Duration::from_mins(20);
const MAX_LENGTH: usize = 1900;
const PREVIEW_CHARS: usize = 200;

/// One step of a running command, read from a task record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Working,
    Said(String),
    ToolStarted { call_id: String, name: String },
    ToolFinished { call_id: String, is_error: bool },
    Finished,
}

impl Step {
    /// The command a task record belongs to and the step it reports.
    pub(crate) fn of(kind: &TaskEventKind) -> (String, Self) {
        match kind {
            TaskEventKind::CommandSubmitted { command } => {
                (command.command_id.to_string(), Self::Working)
            }
            TaskEventKind::ExecutionEvent { command_id, event } => {
                let step = match &event.kind {
                    ExecutionEventKind::AssistantMessage { text } => Self::Said(text.clone()),
                    ExecutionEventKind::ToolStarted { call_id, name, .. } => Self::ToolStarted {
                        call_id: call_id.clone(),
                        name: name.clone(),
                    },
                    ExecutionEventKind::ToolFinished {
                        call_id, is_error, ..
                    } => Self::ToolFinished {
                        call_id: call_id.clone(),
                        is_error: *is_error,
                    },
                    ExecutionEventKind::ExecutionTerminated { .. } => Self::Finished,
                    ExecutionEventKind::ExecutionStarted | ExecutionEventKind::TurnStarted => {
                        Self::Working
                    }
                };
                (command_id.to_string(), step)
            }
        }
    }
}

struct Update {
    command_id: String,
    target: ProgressTarget,
    step: Step,
}

/// The handle the coordinator link reports steps through.
#[derive(Clone)]
pub(crate) struct Progress {
    updates: mpsc::UnboundedSender<Update>,
}

impl Progress {
    pub(crate) fn channel() -> (Self, Receiver) {
        let (updates, receiver) = mpsc::unbounded_channel();
        (Self { updates }, Receiver(receiver))
    }

    pub(crate) fn observe(&self, command_id: String, target: ProgressTarget, step: Step) {
        // A closed receiver means the surface is shutting down.
        let _ = self.updates.send(Update {
            command_id,
            target,
            step,
        });
    }
}

pub(crate) struct Receiver(mpsc::UnboundedReceiver<Update>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolState {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Said(String),
    Tool {
        call_id: String,
        name: String,
        state: ToolState,
    },
}

/// One command's progress.
#[derive(Debug)]
pub(crate) struct Command {
    target: ProgressTarget,
    lines: Vec<Line>,
    /// The latest assistant message. It becomes a line only when a tool call
    /// follows it; otherwise it is the answer, which the reply carries.
    pending: Option<String>,
    /// When the command finished; its progress message is deleted one edit
    /// interval later, once the answer has had time to post.
    finished: Option<Instant>,
    message_id: Option<String>,
    shown: Option<String>,
    touched: Instant,
}

impl Command {
    pub(crate) fn new(target: ProgressTarget) -> Self {
        Self {
            target,
            lines: Vec::new(),
            pending: None,
            finished: None,
            message_id: None,
            shown: None,
            touched: Instant::now(),
        }
    }

    pub(crate) fn apply(&mut self, step: Step) {
        self.touched = Instant::now();
        match step {
            Step::Working => {}
            Step::Said(text) => {
                if let Some(earlier) = self.pending.replace(text) {
                    self.lines.push(Line::Said(earlier));
                }
            }
            Step::ToolStarted { call_id, name } => {
                if let Some(text) = self.pending.take() {
                    self.lines.push(Line::Said(text));
                }
                self.lines.push(Line::Tool {
                    call_id,
                    name,
                    state: ToolState::Running,
                });
            }
            Step::ToolFinished { call_id, is_error } => {
                for line in &mut self.lines {
                    if let Line::Tool {
                        call_id: id, state, ..
                    } = line
                        && *id == call_id
                    {
                        *state = if is_error {
                            ToolState::Failed
                        } else {
                            ToolState::Done
                        };
                    }
                }
            }
            Step::Finished => {
                self.pending = None;
                self.finished = Some(Instant::now());
            }
        }
    }

    fn running(&self) -> bool {
        self.finished.is_none() && self.touched.elapsed() < IDLE_LIMIT
    }

    /// The progress message of a running command, or `None` while there is
    /// nothing beyond typing to show or once the command has finished.
    pub(crate) fn render(&self) -> Option<String> {
        if self.lines.is_empty() || self.finished.is_some() {
            return None;
        }
        let heading = "**Working…**";
        let rendered: Vec<String> = self.lines.iter().map(render_line).collect();
        let mut kept = Vec::new();
        let mut length = heading.len() + 40;
        for line in rendered.iter().rev() {
            if length + line.len() + 1 > MAX_LENGTH {
                break;
            }
            length += line.len() + 1;
            kept.push(line.as_str());
        }
        kept.reverse();
        let mut body = heading.to_owned();
        let hidden = rendered.len() - kept.len();
        if hidden > 0 {
            body.push_str("\n-# … ");
            body.push_str(&hidden.to_string());
            body.push_str(" earlier steps");
        }
        for line in kept {
            body.push('\n');
            body.push_str(line);
        }
        Some(body)
    }
}

fn render_line(line: &Line) -> String {
    match line {
        Line::Said(text) => {
            let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut preview: String = flat.chars().take(PREVIEW_CHARS).collect();
            if flat.chars().count() > PREVIEW_CHARS {
                preview.push('…');
            }
            format!("💭 {preview}")
        }
        Line::Tool { name, state, .. } => {
            let mark = match state {
                ToolState::Running => "…",
                ToolState::Done => "✓",
                ToolState::Failed => "✗",
            };
            format!("🔧 `{name}` {mark}")
        }
    }
}

/// Shows progress until shutdown. Discord failures are logged and never stop
/// the surface.
pub(crate) async fn run(
    api: Arc<DiscordApi>,
    Receiver(mut updates): Receiver,
    shutdown: CancellationToken,
) -> Result<(), DiscordError> {
    let mut commands: HashMap<String, Command> = HashMap::new();
    let mut edits = tokio::time::interval(EDIT_INTERVAL);
    let mut typing = tokio::time::interval(TYPING_INTERVAL);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            update = updates.recv() => {
                let Some(Update { command_id, target, step }) = update else {
                    return Ok(());
                };
                let new = !commands.contains_key(&command_id);
                let command = commands.entry(command_id).or_insert_with(|| Command::new(target));
                command.apply(step);
                if new && command.running() {
                    type_in(&api, &command.target.channel_id).await;
                }
            }
            _ = edits.tick() => show(&api, &mut commands).await,
            _ = typing.tick() => {
                let channels: BTreeSet<&str> = commands
                    .values()
                    .filter(|command| command.running())
                    .map(|command| command.target.channel_id.as_str())
                    .collect();
                for channel in channels {
                    type_in(&api, channel).await;
                }
            }
        }
    }
}

async fn show(api: &DiscordApi, commands: &mut HashMap<String, Command>) {
    for (command_id, command) in commands.iter_mut() {
        if let Some(finished) = command.finished {
            if finished.elapsed() >= EDIT_INTERVAL {
                remove(api, command_id, command).await;
            }
            continue;
        }
        let Some(body) = command.render() else {
            continue;
        };
        if command.shown.as_deref() == Some(body.as_str()) {
            continue;
        }
        let result = match &command.message_id {
            Some(message_id) => api
                .edit_message(&command.target.channel_id, message_id, &body)
                .await
                .map(|()| None),
            None => api
                .create_message(
                    &command.target.channel_id,
                    &body,
                    command.target.reply_to.as_deref(),
                )
                .await
                .map(Some),
        };
        match result {
            Ok(created) => {
                if created.is_some() {
                    command.message_id = created;
                }
                command.shown = Some(body);
            }
            Err(error) => log_failure(command_id, &error.to_string()),
        }
    }
    commands.retain(|_, command| command.running() || command.message_id.is_some());
}

/// Deletes a finished command's progress message. A refusal, such as a
/// message someone already deleted, is final; other failures retry.
async fn remove(api: &DiscordApi, command_id: &str, command: &mut Command) {
    let Some(message_id) = &command.message_id else {
        return;
    };
    match api
        .delete_message(&command.target.channel_id, message_id)
        .await
    {
        Ok(()) => command.message_id = None,
        Err(ApiError::Rejected(error)) => {
            log_failure(command_id, &error);
            command.message_id = None;
        }
        Err(error) => log_failure(command_id, &error.to_string()),
    }
}

async fn type_in(api: &DiscordApi, channel_id: &str) {
    if let Err(error) = api.trigger_typing(channel_id).await {
        log_failure(channel_id, &error.to_string());
    }
}

fn log_failure(subject: &str, error: &str) {
    renoa_telemetry::event(
        "renoa.discord",
        "warn",
        "progress_failed",
        &serde_json::json!({ "subject": subject, "error": error }),
    );
}

#[cfg(test)]
mod tests;
