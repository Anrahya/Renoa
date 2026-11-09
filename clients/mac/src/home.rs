//! Home: the day in a line, what the agents ask, the notes they left while
//! the owner was away, and what runs next, as the phone's Home shows them.
//! Answering a question sends the answer to the agent's chat.

use std::{collections::HashMap, time::Duration};

use gpui_kit::{Context, EventEmitter, IntoElement, Render, SharedString, Task, Window};
use renoa_rcp_wire::{HostMethod, TaskId};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    link::{Command, host::Answer},
    sheets::{
        Host, Loaded, Member, Page,
        routines::held::{Automation, Observation, own},
    },
    theme::Palette,
};
use feed::Held;

pub mod feed;

/// How often Home reads the feed again on its own.
const READ_EVERY: Duration = Duration::from_secs(60);

/// How many routines Home shows coming up.
const COMING: usize = 3;

/// What Home asks of the chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeEvent {
    /// Opens this chat.
    Open(TaskId),
    Sheet(Page),
}

pub struct Home {
    host: Host,
    commands: mpsc::UnboundedSender<Command>,
    members: Vec<Member>,
    feed: Loaded<Held>,
    /// The routines that run next, soonest first.
    coming: Vec<Automation>,
    reading: bool,
    /// The run asked of each failed routine the Host has not confirmed, by
    /// its feed item: asked again, it is the same run.
    runs: HashMap<Uuid, Uuid>,
    /// Something that did not go through, said once.
    refused: Option<SharedString>,
    here_zone: Option<String>,
    _reads: Task<()>,
}

impl EventEmitter<HomeEvent> for Home {}

impl Home {
    pub fn new(
        commands: mpsc::UnboundedSender<Command>,
        members: Vec<Member>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _reads = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |home, cx| home.read(cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(READ_EVERY).await;
            }
        });
        Self {
            host: Host(commands.clone()),
            commands,
            members,
            feed: Loaded::Waiting,
            coming: Vec::new(),
            reading: false,
            runs: HashMap::new(),
            refused: None,
            here_zone: iana_time_zone::get_timezone().ok(),
            _reads,
        }
    }

    pub fn set_members(&mut self, members: Vec<Member>, cx: &mut Context<Self>) {
        self.members = members;
        cx.notify();
    }

    /// How many things wait on the owner, once the feed is read.
    pub fn needing_you(&self) -> Option<usize> {
        match &self.feed {
            Loaded::Ready(held) => Some(held.needing_you()),
            Loaded::Waiting | Loaded::Unread => None,
        }
    }

    /// Reads the feed, every page of it, and the routines; a read already
    /// on its way is enough.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let first = host.get::<feed::Page>(feed::page_path(None)).await;
            let read = match first {
                Answer::Said(first) => {
                    let mut items = first.items.clone();
                    let mut at = feed::next(None, &first);
                    let mut whole = true;
                    while let Some(before) = at {
                        match host.get::<feed::Page>(feed::page_path(Some(before))).await {
                            Answer::Said(older) => {
                                at = feed::next(Some(before), &older);
                                items.extend(older.items);
                            }
                            _ => {
                                whole = false;
                                break;
                            }
                        }
                    }
                    whole.then_some((first.today, items))
                }
                _ => None,
            };
            let coming = host.get::<Observation>("/v1/host".to_owned()).await;
            let _ = this.update(cx, |home, cx| {
                home.reading = false;
                match (read, &mut home.feed) {
                    (Some((today, items)), Loaded::Ready(held)) => held.read(today, items),
                    (Some((today, items)), shown) => {
                        let mut held = Held::default();
                        held.read(today, items);
                        *shown = Loaded::Ready(held);
                    }
                    // What is shown stays while Renoa is out of reach.
                    (None, Loaded::Ready(_)) => {}
                    (None, shown) => *shown = Loaded::Unread,
                }
                if let Answer::Said(observed) = coming {
                    home.coming = soonest(own(observed.automations));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn held(&mut self) -> Option<&mut Held> {
        match &mut self.feed {
            Loaded::Ready(held) => Some(held),
            Loaded::Waiting | Loaded::Unread => None,
        }
    }

    fn member(&self, agent: Uuid) -> Option<&Member> {
        self.members.iter().find(|member| member.host_id == agent)
    }

    /// Sends `choice` to the asking agent's chat, naming the question when it
    /// was asked in another conversation, which is then put away. The
    /// question hides at once and shows again if Renoa refuses the answer.
    fn answer(&mut self, id: Uuid, choice: String, cx: &mut Context<Self>) {
        let Some(ask) = self.held().and_then(|held| held.item(id)).cloned() else {
            return;
        };
        let Some(chat) = self.member(ask.agent.id).map(|member| member.task_id) else {
            return;
        };
        let elsewhere = Uuid::parse_str(&ask.subject_id).ok() != Some(chat.as_uuid());
        let text = if elsewhere {
            feed::from_elsewhere(&ask.body, &choice)
        } else {
            choice
        };
        let (sent, heard) = oneshot::channel();
        if self
            .commands
            .send(Command::Send {
                task_id: chat,
                text,
                sent: Some(sent),
            })
            .is_err()
        {
            return;
        }
        if let Some(held) = self.held() {
            held.deal(id);
        }
        self.refused = None;
        cx.notify();
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let took = heard.await.unwrap_or(false);
            if took && elsewhere {
                // The answer is sent; a question left open on the Host shows
                // again on the next read.
                let _ = host.call(HostMethod::Post, dismissal(id), None).await;
            }
            let _ = this.update(cx, |home, cx| {
                if !took {
                    if let Some(held) = home.held() {
                        held.restore(id);
                    }
                    home.refused = Some("Renoa couldn't send that answer. Try again.".into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dismiss(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(held) = self.held() else {
            return;
        };
        held.deal(id);
        self.refused = None;
        cx.notify();
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let answer = host.call(HostMethod::Post, dismissal(id), None).await;
            let _ = this.update(cx, |home, cx| {
                if !matches!(answer, Answer::Said(_) | Answer::Missing) {
                    if let Some(held) = home.held() {
                        held.restore(id);
                    }
                    home.refused = Some("Renoa couldn't put that away just now.".into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs a failed routine again and hides its note; a run that fails
    /// again shows a new one on a later read.
    fn run_again(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(routine) = self
            .held()
            .and_then(|held| held.item(id))
            .map(|problem| problem.subject_id.clone())
        else {
            return;
        };
        let operation_id = *self.runs.entry(id).or_insert_with(Uuid::new_v4);
        if let Some(held) = self.held() {
            held.deal(id);
        }
        self.refused = None;
        cx.notify();
        let body = serde_json::json!({ "operation_id": operation_id }).to_string();
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let path = format!("/v1/host/automations/{routine}/run");
            let answer = host.call(HostMethod::Post, path, Some(body)).await;
            let _ = this.update(cx, |home, cx| {
                if matches!(answer, Answer::Later | Answer::Lost) {
                    if let Some(held) = home.held() {
                        held.restore(id);
                    }
                    home.refused = Some(
                        "Renoa couldn't confirm that routine started. Try again in a moment."
                            .into(),
                    );
                } else {
                    home.runs.remove(&id);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn dismissal(id: Uuid) -> String {
    format!("/v1/host/feed/items/{id}/dismissal")
}

/// The routines that are on, soonest first, as many as Home shows.
fn soonest(mut routines: Vec<Automation>) -> Vec<Automation> {
    routines.retain(|routine| routine.enabled);
    routines.sort_by_key(|routine| routine.next_due_ms);
    routines.truncate(COMING);
    routines
}

mod view;

impl Render for Home {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        view::render(self, Palette::of(window, cx), cx)
    }
}

#[cfg(test)]
mod tests;
