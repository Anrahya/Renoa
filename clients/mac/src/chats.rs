//! The chats: the owner's agents down the side, one conversation beside them.

use std::{
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window,
    component::{
        input::{InputEvent, InputState},
        message_scroller::MessageScrollerState,
    },
    div, px,
};
use renoa_rcp_wire::{TargetSummary, TaskId};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    chat::{Line, Speaker, Transcript, chat_task_id},
    face::Look,
    link::{Command, Update, host_agent},
    theme::Palette,
};

/// What a row of the conversation shows.
struct Row {
    /// `None` past the last line: the agent at work.
    line: Option<Line>,
    before: Option<Line>,
    /// Who says the next line, if anyone has.
    after: Option<Speaker>,
    /// Whether it arrived while the chat was open.
    live: bool,
}

/// Renoa no longer knows this Mac.
pub struct SignedOut;

/// The chat the owner has open. It stays open while its agent is offline.
struct Chosen {
    task_id: TaskId,
    name: SharedString,
}

pub struct Chats {
    commands: mpsc::UnboundedSender<Command>,
    linked: bool,
    agents: Vec<TargetSummary>,
    chosen: Option<Chosen>,
    transcripts: HashMap<TaskId, Transcript>,
    /// What each chat's agent is for, as its owner wrote it.
    purposes: HashMap<TaskId, SharedString>,
    /// How each chat's agent looks, as the Host picked it.
    looks: HashMap<TaskId, Look>,
    /// The Host's main agent.
    main: Option<Uuid>,
    /// What the owner typed in each chat they are not looking at.
    drafts: HashMap<TaskId, String>,
    /// The chats where Renoa refused the owner's last message.
    unsent: HashSet<TaskId>,
    composer: Entity<InputState>,
    scroller: Entity<MessageScrollerState>,
    /// Whether the scroller's last row is the agent at work.
    shown_working: bool,
    /// The first row that arrived while the chat was open: it and those
    /// after it come in with a motion, the ones before were already there.
    live_from: usize,
    _updates: Task<()>,
}

impl EventEmitter<SignedOut> for Chats {}

impl Chats {
    /// Shows what `updates` says and asks `commands` for chats and messages.
    pub fn new(
        commands: mpsc::UnboundedSender<Command>,
        mut updates: mpsc::UnboundedReceiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send(window, cx);
                }
            },
        )
        .detach();
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let _updates = cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = updates.recv().await {
                let applied = this.update_in(cx, |this, window, cx| this.apply(update, window, cx));
                if applied.is_err() {
                    return;
                }
            }
        });
        Self {
            commands,
            linked: false,
            agents: Vec::new(),
            chosen: None,
            transcripts: HashMap::new(),
            purposes: HashMap::new(),
            looks: HashMap::new(),
            main: None,
            drafts: HashMap::new(),
            unsent: HashSet::new(),
            composer,
            scroller,
            shown_working: false,
            live_from: 0,
            _updates,
        }
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Linked(linked) => self.linked = linked,
            Update::Agents(agents) => {
                self.agents = agents;
                match &mut self.chosen {
                    // A renamed agent shows its new name.
                    Some(chosen) => {
                        if let Some(agent) = self.agents.iter().find(|agent| {
                            chat_task_id(agent.node_id, agent.target.as_str()) == chosen.task_id
                        }) {
                            chosen.name = agent.name.clone().into();
                        }
                    }
                    None => self.select(0, window, cx),
                }
            }
            Update::Purpose {
                node_id,
                target,
                purpose,
            } => {
                self.purposes
                    .insert(chat_task_id(node_id, &target), purpose.into());
            }
            Update::Look {
                node_id,
                target,
                look,
            } => {
                self.looks.insert(chat_task_id(node_id, &target), look);
            }
            Update::Main(main) => self.main = main,
            Update::Event(event) => {
                self.transcripts
                    .entry(event.task_id)
                    .or_default()
                    .apply(&event);
                if self.chosen_task() == Some(event.task_id) {
                    self.show_rows(cx);
                }
            }
            Update::NotSent { task_id, text } => {
                self.unsent.insert(task_id);
                // Give the text back, ahead of anything the owner typed since.
                if self.chosen_task() == Some(task_id) {
                    let typed = self.composer.read(cx).value().to_string();
                    let restored = joined(text, &typed);
                    self.composer
                        .update(cx, |composer, cx| composer.set_value(restored, window, cx));
                } else {
                    let draft = self.drafts.remove(&task_id).unwrap_or_default();
                    self.drafts.insert(task_id, joined(text, &draft));
                }
            }
            Update::SignedOut => cx.emit(SignedOut),
        }
        cx.notify();
    }

    fn chosen_task(&self) -> Option<TaskId> {
        self.chosen.as_ref().map(|chosen| chosen.task_id)
    }

    fn transcript(&self) -> Option<&Transcript> {
        self.chosen_task()
            .and_then(|task_id| self.transcripts.get(&task_id))
    }

    /// Opens the chat with the agent at `index`, keeping what the owner typed
    /// in the chat they leave.
    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agents.get(index) else {
            return;
        };
        let task_id = chat_task_id(agent.node_id, agent.target.as_str());
        if self.chosen_task() == Some(task_id) {
            return;
        }
        let after = self.transcripts.get(&task_id).and_then(Transcript::through);
        let _ = self.commands.send(Command::Follow {
            task_id,
            node_id: agent.node_id,
            target: agent.target.clone(),
            after,
        });
        let name = agent.name.clone().into();
        let typed = self.composer.read(cx).value().to_string();
        let mut draft = self.drafts.remove(&task_id).unwrap_or_default();
        match self.chosen_task() {
            Some(left) if !typed.is_empty() => {
                self.drafts.insert(left, typed);
            }
            Some(_) => {}
            // Typed before any chat was open: it goes to the first one.
            None => draft = joined(draft, &typed),
        }
        self.chosen = Some(Chosen { task_id, name });
        self.composer
            .update(cx, |composer, cx| composer.set_value(draft, window, cx));
        let rows = self.rows();
        self.shown_working = self.transcript().is_some_and(Transcript::working);
        self.live_from = self
            .transcript()
            .map_or(0, |transcript| transcript.lines().len());
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(rows, cx));
        cx.notify();
    }

    /// What a chat's row says under its name: the last thing said, or
    /// before anything is, what the agent is for.
    fn preview(&self, task_id: TaskId) -> SharedString {
        self.transcripts
            .get(&task_id)
            .and_then(|transcript| transcript.lines().last())
            .map(|line| line.text.clone().into())
            .or_else(|| self.purposes.get(&task_id).cloned())
            .unwrap_or_default()
    }

    /// The rows the conversation shows: its lines, then a working row while
    /// the agent answers.
    fn rows(&self) -> usize {
        self.transcript().map_or(0, |transcript| {
            transcript.lines().len() + usize::from(transcript.working())
        })
    }

    /// What row `index` shows: a line, with the one before it; past the
    /// last line, the agent at work.
    fn row(&self, index: usize) -> Row {
        let lines = self.transcript().map_or(&[][..], Transcript::lines);
        Row {
            line: lines.get(index).cloned(),
            before: index
                .checked_sub(1)
                .and_then(|before| lines.get(before))
                .cloned(),
            after: lines.get(index + 1).map(|after| after.speaker),
            live: index >= self.live_from && lines.get(index).is_none_or(fresh),
        }
    }

    /// How the agent of `agent` looks, and whether it is the main agent.
    fn face(&self, agent: &TargetSummary) -> (Option<Look>, bool) {
        let task_id = chat_task_id(agent.node_id, agent.target.as_str());
        let primary = host_agent(agent.target.as_str()).is_some_and(|id| Some(id) == self.main);
        (self.looks.get(&task_id).copied(), primary)
    }

    /// Tells the scroller what changed: rows after the old working row are
    /// new, and the working row itself became a line or went away.
    fn show_rows(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows();
        let working = self.transcript().is_some_and(Transcript::working);
        let shown_working = std::mem::replace(&mut self.shown_working, working);
        self.scroller.update(cx, |scroller, cx| {
            let shown = scroller.item_count();
            let settled = shown - usize::from(shown_working);
            if rows < settled {
                self.live_from = rows;
                scroller.reset(rows, cx);
            } else if (rows, working) != (shown, shown_working) {
                scroller.splice(settled..shown, rows - settled, cx);
            }
        });
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_owned();
        let Some(task_id) = self.chosen_task() else {
            return;
        };
        // Offline, the text stays in the composer until Renoa is back.
        if text.is_empty() || !self.linked {
            return;
        }
        let _ = self.commands.send(Command::Send { task_id, text });
        self.unsent.remove(&task_id);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }
}

impl Render for Chats {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window);
        div()
            .size_full()
            .flex()
            .bg(palette.background)
            .text_color(palette.text)
            .text_size(px(13.))
            .child(self.sidebar(palette, cx))
            .child(self.conversation(palette, cx))
    }
}

/// How long after it was said a line still comes in with a motion: a replay
/// of older lines, on first opening a chat, comes in still.
const FRESH_FOR_MS: i64 = 30_000;

/// Whether `line` was said within [`FRESH_FOR_MS`], or has no time yet.
fn fresh(line: &Line) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    line.at_ms.is_none_or(|at_ms| now - at_ms < FRESH_FOR_MS)
}

/// `first`, then `then` after a space when there is any.
fn joined(first: String, then: &str) -> String {
    match (first.is_empty(), then.is_empty()) {
        (_, true) => first,
        (true, false) => then.to_owned(),
        (false, false) => format!("{first} {then}"),
    }
}

mod sidebar;
mod stamp;
#[cfg(test)]
mod tests;
mod thread;
