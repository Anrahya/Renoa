//! The chats: the owner's agents down the side, one conversation beside them.

use std::collections::{HashMap, HashSet};

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Task, Window,
    component::{
        input::{Input, InputEvent, InputState},
        message_scroller::{MessageScroller, MessageScrollerState},
    },
    div,
    prelude::FluentBuilder as _,
    px, relative,
};
use renoa_rcp_wire::{TargetSummary, TaskId};
use tokio::sync::mpsc;

use crate::{
    chat::{Line, Speaker, Transcript, chat_task_id},
    link::{Command, Update},
    theme::Palette,
};

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
    /// What the owner typed in each chat they are not looking at.
    drafts: HashMap<TaskId, String>,
    /// The chats where Renoa refused the owner's last message.
    unsent: HashSet<TaskId>,
    composer: Entity<InputState>,
    scroller: Entity<MessageScrollerState>,
    /// Whether the scroller's last row is the agent at work.
    shown_working: bool,
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
            drafts: HashMap::new(),
            unsent: HashSet::new(),
            composer,
            scroller,
            shown_working: false,
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
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(rows, cx));
        cx.notify();
    }

    /// The rows the conversation shows: its lines, then a working row while
    /// the agent answers.
    fn rows(&self) -> usize {
        self.transcript().map_or(0, |transcript| {
            transcript.lines().len() + usize::from(transcript.working())
        })
    }

    /// The line at row `index`; `None` past the last line, the agent at work.
    fn row(&self, index: usize) -> Option<Line> {
        self.transcript()
            .and_then(|transcript| transcript.lines().get(index).cloned())
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

impl Chats {
    fn sidebar(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.agents.iter().enumerate().map(|(index, agent)| {
            let task_id = chat_task_id(agent.node_id, agent.target.as_str());
            let preview: SharedString = self
                .transcripts
                .get(&task_id)
                .and_then(|transcript| transcript.lines().last())
                .map_or_else(SharedString::default, |line| line.text.clone().into());
            div()
                .id(("agent", index))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(10.))
                .cursor_pointer()
                .when(self.chosen_task() == Some(task_id), |row| {
                    row.bg(palette.selected)
                })
                .on_click(cx.listener(move |this, _, window, cx| this.select(index, window, cx)))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(agent.name.clone())),
                )
                .child(
                    div()
                        .text_color(palette.secondary)
                        .text_size(px(12.))
                        .truncate()
                        .child(preview),
                )
        });
        div()
            .w(px(240.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(8.))
            // Room for the window's traffic lights.
            .pt(px(44.))
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.outline)
            .children(rows)
            .when(self.agents.is_empty(), |side| {
                side.child(
                    div()
                        .px(px(10.))
                        .text_color(palette.secondary)
                        .child(if self.linked {
                            "No agents yet. Make one in Renoa on your phone."
                        } else {
                            "Connecting to Renoa…"
                        }),
                )
            })
    }

    fn conversation(&self, palette: Palette, cx: &Context<Self>) -> impl IntoElement {
        let name: SharedString = self
            .chosen
            .as_ref()
            .map_or_else(SharedString::default, |chosen| chosen.name.clone());
        let notice = self
            .chosen_task()
            .filter(|task_id| self.unsent.contains(task_id))
            .map(|_| "Your message didn't send. Try again.");
        // Rows read the chat as they are drawn; only the visible ones are.
        let chats = cx.entity().downgrade();
        let agent_name = name.clone();
        let thread = MessageScroller::new("thread", self.scroller.clone(), move |index, _, cx| {
            let line = chats.upgrade().and_then(|chats| chats.read(cx).row(index));
            line_row(line.as_ref(), &agent_name, palette)
        })
        .flex_1();
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .when(!name.is_empty(), |header| {
                        header.child(
                            div()
                                .px(px(14.))
                                .py(px(6.))
                                .rounded_full()
                                .bg(palette.frosted)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                    })
                    .when(!self.linked, |header| {
                        header.child(
                            div()
                                .text_color(palette.attention)
                                .text_size(px(12.))
                                .child("Reconnecting…"),
                        )
                    }),
            )
            .child(thread)
            .when_some(notice, |column, notice| {
                column.child(
                    div()
                        .px(px(20.))
                        .text_color(palette.danger)
                        .text_size(px(12.))
                        .child(notice),
                )
            })
            .child(
                div()
                    .m(px(12.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(22.))
                    .bg(palette.composer)
                    .border_1()
                    .border_color(palette.outline)
                    .child(
                        Input::new(&self.composer)
                            .id("composer")
                            .aria_label("Message")
                            .appearance(false),
                    ),
            )
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

/// One row of the conversation; past the last line, the agent at work.
fn line_row(line: Option<&Line>, agent: &SharedString, palette: Palette) -> gpui_kit::Div {
    let row = div().w_full().flex().px(px(20.)).py(px(4.));
    let Some(line) = line else {
        return row.child(
            div()
                .text_color(palette.secondary)
                .child(format!("{agent} is thinking…")),
        );
    };
    let bubble = div()
        .max_w(relative(0.72))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(16.))
        .child(SharedString::from(line.text.clone()));
    match line.speaker {
        Speaker::Owner => row.justify_end().child(bubble.bg(palette.chat_user)),
        Speaker::Agent => row.child(
            bubble
                .bg(palette.chat_agent)
                .when(line.failed, |bubble| bubble.text_color(palette.danger)),
        ),
    }
}

#[cfg(test)]
mod tests;
