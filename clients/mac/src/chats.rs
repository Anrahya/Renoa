//! The chats: the owner's agents down the side, one conversation beside them.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    component::{
        input::{InputEvent, InputState},
        message_scroller::MessageScrollerState,
    },
    div,
};
use renoa_rcp_wire::{TargetSummary, TaskId};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    chat::{Line, Transcript, chat_task_id},
    face::{Look, blink},
    home::Home,
    link::{Command, Update, host_agent},
    theme::Palette,
};

use crate::theme::u;

/// What a row of the conversation shows.
struct Row {
    /// `None` past the last line: the agent at work.
    line: Option<Line>,
    before: Option<Line>,
    /// Whether it is coming in.
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
    /// The clock the faces blink by.
    opened: Instant,
    _updates: Task<()>,
    _blinks: Task<()>,
    /// What this Mac calls itself.
    computer: SharedString,
    /// Holds the keys while a sheet is up: the scrim under it tracks it.
    focus: FocusHandle,
    sheet: Option<overlay::Shown>,
    jumping: Option<overlay::Jumping>,
    /// How many sheets have opened, so each rises afresh.
    sheets_opened: usize,
    home: Entity<Home>,
    /// Whether Home shows rather than the chosen chat.
    on_home: bool,
    /// Holds the keys while Home shows: Home tracks it.
    home_focus: FocusHandle,
    _home_events: Subscription,
}

impl EventEmitter<SignedOut> for Chats {}

impl Chats {
    /// Shows what `updates` says and asks `commands` for chats and messages.
    pub fn new(
        commands: mpsc::UnboundedSender<Command>,
        mut updates: mpsc::UnboundedReceiver<Update>,
        computer: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let home = cx.new(|cx| Home::new(commands.clone(), Vec::new(), cx));
        let _home_events = cx.subscribe_in(&home, window, Self::on_home);
        // The keys reach the chats through whatever has focus inside them,
        // so something must from the start: Home, which shows first.
        let home_focus = cx.focus_handle();
        window.focus(&home_focus, cx);
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
        // Draws again only as an eye opens or shuts; a list that changes
        // meanwhile is caught on the next wake.
        let _blinks = cx.spawn_in(window, async move |this, cx| {
            while let Ok(wait) = this.update(cx, |this, _| this.next_blink()) {
                cx.background_executor().timer(wait).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
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
            opened: Instant::now(),
            _updates,
            _blinks,
            computer,
            focus: cx.focus_handle(),
            sheet: None,
            jumping: None,
            sheets_opened: 0,
            home,
            on_home: true,
            home_focus,
            _home_events,
        }
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::Linked(linked) => {
                self.linked = linked;
                self.home.update(cx, |home, cx| home.set_linked(linked, cx));
            }
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
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| {
                        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                    });
                let timed = self
                    .transcripts
                    .entry(event.task_id)
                    .or_default()
                    .apply(&event, now_ms);
                if self.chosen_task() == Some(event.task_id) {
                    self.show_rows(cx);
                    // A line that took a time may now show one above it.
                    if let Some(timed) = timed {
                        self.scroller
                            .update(cx, |scroller, cx| scroller.remeasure_items(timed, cx));
                    }
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
        // Home shows each agent as the chats do.
        let members = self.members();
        self.home
            .update(cx, |home, cx| home.set_members(members, cx));
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
        let transcript = self.transcript();
        let lines = transcript.map_or(&[][..], Transcript::lines);
        Row {
            line: lines.get(index).cloned(),
            before: index
                .checked_sub(1)
                .and_then(|before| lines.get(before))
                .cloned(),
            live: transcript.is_some_and(|transcript| transcript.coming_in(index)),
        }
    }

    /// Whether the agent of `task_id`'s chat has its eyes shut just now.
    fn blinking(&self, task_id: TaskId) -> bool {
        blink::shut(task_id.as_uuid().as_u128(), self.blink_clock())
    }

    fn blink_clock(&self) -> u64 {
        u64::try_from(self.opened.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// How long until any listed agent's eyes open or shut.
    fn next_blink(&self) -> Duration {
        let seeds = self.agents.iter().map(|agent| {
            chat_task_id(agent.node_id, agent.target.as_str())
                .as_uuid()
                .as_u128()
        });
        blink::next_change(seeds, self.blink_clock())
    }

    /// How the agent of `agent` looks, and whether it is the main agent: the
    /// one the Host names, or while it names none that is listed, the first
    /// listed, as on the phone (`mainAgent` in the Android app's `Badge.kt`).
    fn face(&self, agent: &TargetSummary) -> (Option<Look>, bool) {
        let task_id = chat_task_id(agent.node_id, agent.target.as_str());
        let named = |agent: &TargetSummary| {
            host_agent(agent.target.as_str()).is_some_and(|id| Some(id) == self.main)
        };
        let main = self
            .agents
            .iter()
            .find(|listed| named(listed))
            .or_else(|| self.agents.first());
        let primary =
            main.is_some_and(|main| main.node_id == agent.node_id && main.target == agent.target);
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
            // A transcript only grows, so the rows never fall below those settled.
            let settled = shown - usize::from(shown_working);
            if (rows, working) != (shown, shown_working) {
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
        let _ = self.commands.send(Command::Send {
            task_id,
            text,
            sent: None,
        });
        self.unsent.remove(&task_id);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }
}

impl Render for Chats {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let keys = cx.listener(|this, event, window, cx| {
            if this.key(event, window, cx) {
                cx.stop_propagation();
            }
        });
        let sidebar = self.sidebar(palette, cx);
        let main = if self.on_home {
            self.home_pane().into_any_element()
        } else {
            self.conversation(palette, window, cx).into_any_element()
        };
        let overlay = self.overlay(palette, cx);
        let root = div().size_full().relative().flex();
        self.answer_keys(root, cx)
            .capture_key_down(keys)
            .bg(palette.background)
            .text_color(palette.text)
            .text_size(u(13.))
            .child(sidebar)
            .child(main)
            .children(overlay)
    }
}

/// `first`, then `then` after a space when there is any.
fn joined(first: String, then: &str) -> String {
    match (first.is_empty(), then.is_empty()) {
        (_, true) => first,
        (true, false) => then.to_owned(),
        (false, false) => format!("{first} {then}"),
    }
}

mod home;
mod jump;
pub(crate) mod keys;
mod overlay;
#[cfg(test)]
mod screens;
mod sidebar;
#[cfg(test)]
mod tests;
mod thread;
