//! Transient progress for running commands: a typing indicator in the
//! command's channel and one progress message, edited in place, listing its
//! tool calls and the intermediate messages that led to them. One edit interval
//! after the command finishes, or once it has been idle too long, the progress
//! message is deleted.
//!
//! Progress is presence, not history. Only the posted message's identity is
//! stored, so a restarted surface still deletes it: after a restart progress
//! resumes from the next task record in the same message, and a finished
//! command's message is deleted. A message posted just before a crash, and not
//! yet recorded, is left behind. A failed Discord call is retried on the next
//! tick or dropped. A command that calls no tool shows only the typing
//! indicator, and a command that finishes before its progress message was
//! posted never posts one.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

use renoa_control::TaskEventKind;
use renoa_protocol::ExecutionEventKind;
use tokio::{
    sync::mpsc,
    time::{Interval, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::{
    DiscordError,
    api::{ApiError, DiscordApi},
    store::{ProgressTarget, ShownProgress, SurfaceStore},
};

/// Discord shows a typing indicator for about ten seconds.
const TYPING_INTERVAL: Duration = Duration::from_secs(8);
/// Edits stay well inside Discord's per-channel message rate limit.
const EDIT_INTERVAL: Duration = Duration::from_millis(1500);
/// A command that reports nothing for this long is no longer shown as working,
/// and its progress message is deleted.
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
                    ExecutionEventKind::ToolStarted {
                        call_id,
                        name,
                        arguments,
                    } => Self::ToolStarted {
                        call_id: call_id.clone(),
                        name: label::tool_label(name, arguments),
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
    /// interval later, so the answer usually posts first.
    finished: Option<Instant>,
    message_id: Option<String>,
    shown: Option<String>,
    touched: Instant,
}

impl Command {
    pub(crate) fn new(target: ProgressTarget, now: Instant) -> Self {
        Self {
            target,
            lines: Vec::new(),
            pending: None,
            finished: None,
            message_id: None,
            shown: None,
            touched: now,
        }
    }

    /// A command whose progress message a previous run posted.
    fn resumed(shown: ShownProgress, now: Instant) -> (String, Self) {
        let mut command = Self::new(
            ProgressTarget {
                channel_id: shown.channel_id,
                reply_to: None,
            },
            now,
        );
        command.message_id = Some(shown.message_id);
        if shown.finished {
            command.finished = Some(now);
        }
        (shown.command_id, command)
    }

    pub(crate) fn apply(&mut self, step: Step, now: Instant) {
        self.touched = now;
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
                self.finished = Some(now);
            }
        }
    }

    fn running(&self, now: Instant) -> bool {
        self.finished.is_none() && now.saturating_duration_since(self.touched) < IDLE_LIMIT
    }

    /// Whether the command's progress message should be deleted: one edit
    /// interval after it finished, or once it has been idle too long.
    fn expired(&self, now: Instant) -> bool {
        match self.finished {
            Some(finished) => now.saturating_duration_since(finished) >= EDIT_INTERVAL,
            None => now.saturating_duration_since(self.touched) >= IDLE_LIMIT,
        }
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
        // Room for the heading and the "… N earlier steps" line.
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
    store: Arc<SurfaceStore>,
    Receiver(mut updates): Receiver,
    shutdown: CancellationToken,
) -> Result<(), DiscordError> {
    let mut commands = resume(&store, Instant::now())?;
    let mut edits = ticker(EDIT_INTERVAL);
    let mut typing = ticker(TYPING_INTERVAL);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            update = updates.recv() => {
                let Some(Update { command_id, target, step }) = update else {
                    return Ok(());
                };
                let now = Instant::now();
                let new = !commands.contains_key(&command_id);
                let command = commands
                    .entry(command_id)
                    .or_insert_with(|| Command::new(target, now));
                command.apply(step, now);
                if new && command.running(now) {
                    type_in(&api, &command.target.channel_id).await;
                }
            }
            _ = edits.tick() => show(&api, &store, &mut commands, Instant::now()).await,
            _ = typing.tick() => {
                let now = Instant::now();
                let channels: BTreeSet<&str> = commands
                    .values()
                    .filter(|command| command.running(now))
                    .map(|command| command.target.channel_id.as_str())
                    .collect();
                for channel in channels {
                    type_in(&api, channel).await;
                }
            }
        }
    }
}

/// The commands whose progress messages a previous run posted and did not
/// delete.
fn resume(store: &SurfaceStore, now: Instant) -> Result<HashMap<String, Command>, DiscordError> {
    Ok(store
        .shown_progress()?
        .into_iter()
        .map(|shown| Command::resumed(shown, now))
        .collect())
}

/// A timer that, after a stall, ticks once and then keeps its period instead
/// of catching up on every missed tick with a burst of Discord calls.
fn ticker(period: Duration) -> Interval {
    let mut timer = tokio::time::interval(period);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    timer
}

async fn show(
    api: &DiscordApi,
    store: &SurfaceStore,
    commands: &mut HashMap<String, Command>,
    now: Instant,
) {
    for (command_id, command) in commands.iter_mut() {
        if command.expired(now) {
            remove(api, store, command_id, command).await;
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
                if let Some(message_id) = created {
                    if let Err(error) = store.record_progress_message(
                        command_id,
                        &command.target.channel_id,
                        &message_id,
                    ) {
                        log_failure(command_id, &error.to_string());
                    }
                    command.message_id = Some(message_id);
                }
                command.shown = Some(body);
            }
            Err(error) => log_failure(command_id, &error.to_string()),
        }
    }
    commands.retain(|_, command| !command.expired(now) || command.message_id.is_some());
}

/// Deletes an expired command's progress message. A refusal, such as a
/// message someone already deleted, is final; other failures retry.
async fn remove(api: &DiscordApi, store: &SurfaceStore, command_id: &str, command: &mut Command) {
    let Some(message_id) = &command.message_id else {
        return;
    };
    match api
        .delete_message(&command.target.channel_id, message_id)
        .await
    {
        Ok(()) => {}
        Err(ApiError::Rejected(error)) => log_failure(command_id, &error),
        Err(error) => return log_failure(command_id, &error.to_string()),
    }
    command.message_id = None;
    if let Err(error) = store.clear_progress_message(command_id) {
        log_failure(command_id, &error.to_string());
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

mod label;

#[cfg(test)]
mod tests;
