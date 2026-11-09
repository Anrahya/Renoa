//! An agent's page: who it is, what it is for, the routines it runs, the
//! apps it uses and what it has been up to. It only shows: the Host cannot
//! change an agent from an app yet, so changes happen in the agent's chat
//! and on the Routines and Apps sheets.

use chrono::Local;
use gpui_kit::{
    AnyElement, Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestSupportExt as _, Window, assets::IconName, div, prelude::FluentBuilder as _, px,
};
use serde::Deserialize;

use super::{
    Host, Loaded, Member, Page, SheetEvent,
    apps::{AgentApp, Apps, label},
    group, mark, not_ready, pill,
    routines::held::{Automation, Observation, own},
    schedule, section,
};
use crate::{
    face::Face,
    home::feed::{self, Item, Kind},
    link::host::Answer,
    stamp,
    theme::Palette,
};

/// How many of its latest notes the page shows.
const LATELY: usize = 4;

/// The field of the Host's agent definition this page reads; what the
/// agent is for comes from the chats, which read it once for every agent.
#[derive(Debug, Deserialize)]
struct Definition {
    created_at_ms: i64,
}

/// What the page shows of the agent, as the Host last said. Each part is
/// `None` when it could not be read; the rest still shows.
#[derive(Debug)]
struct Held {
    since_ms: i64,
    routines: Option<Vec<Automation>>,
    apps: Option<Vec<AgentApp>>,
    /// Its latest notes and questions, newest first.
    lately: Option<Vec<Item>>,
}

pub struct Profile {
    host: Host,
    member: Member,
    purpose: Option<SharedString>,
    loaded: Loaded<Held>,
    /// The Host no longer has the agent.
    gone: bool,
    reading: bool,
    here_zone: Option<String>,
}

impl EventEmitter<SheetEvent> for Profile {}

impl Profile {
    pub fn new(
        host: Host,
        member: Member,
        purpose: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut profile = Self {
            host,
            member,
            purpose,
            loaded: Loaded::Waiting,
            gone: false,
            reading: false,
            here_zone: iana_time_zone::get_timezone().ok(),
        };
        profile.read(cx);
        profile
    }

    /// What the page says the agent is for.
    #[cfg(test)]
    pub fn purpose(&self) -> Option<&SharedString> {
        self.purpose.as_ref()
    }

    /// Reads the agent, then each part of the page in turn; a read already
    /// on its way is enough, and one for a page that closed stops.
    fn read(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        let host = self.host.clone();
        let agent = self.member.host_id;
        cx.spawn(async move |this, cx| {
            let open = || this.upgrade().is_some();
            let definition = host
                .get::<Definition>(format!("/v1/host/agents/{agent}"))
                .await;
            let read = match definition {
                Answer::Said(definition) if open() => {
                    let apps = match host
                        .get::<Apps>(format!("/v1/host/agents/{agent}/apps"))
                        .await
                    {
                        Answer::Said(apps) => {
                            Some(apps.apps.into_iter().filter(|app| app.on).collect())
                        }
                        // Gone meanwhile: it has no apps.
                        Answer::Missing => Some(Vec::new()),
                        Answer::Conflict | Answer::Later | Answer::Lost => None,
                    };
                    let routines = match host.get::<Observation>("/v1/host".to_owned()).await {
                        Answer::Said(observed) => Some(its_routines(observed.automations, agent)),
                        _ => None,
                    };
                    let mut lately = Some(Vec::new());
                    let mut at = None;
                    while let Some(found) = lately.as_mut().filter(|_| open()) {
                        let Answer::Said(page) = host.get::<feed::Page>(feed::page_path(at)).await
                        else {
                            lately = None;
                            break;
                        };
                        at = feed::next(at, &page);
                        found.extend(page.items.into_iter().filter(|item| mine(item, agent)));
                        if found.len() >= LATELY || at.is_none() {
                            found.truncate(LATELY);
                            break;
                        }
                    }
                    Some(Loaded::Ready(Held {
                        since_ms: definition.created_at_ms,
                        routines,
                        apps,
                        lately,
                    }))
                }
                Answer::Said(_) => return,
                Answer::Missing => None,
                Answer::Conflict | Answer::Later | Answer::Lost => Some(Loaded::Unread),
            };
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                match read {
                    Some(loaded) => this.loaded = loaded,
                    None => this.gone = true,
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The routines `agent` runs, the ones that are on first, each group soonest
/// first.
fn its_routines(automations: Vec<Automation>, agent: uuid::Uuid) -> Vec<Automation> {
    let mut routines: Vec<Automation> = own(automations)
        .into_iter()
        .filter(|routine| routine.agent_id == agent)
        .collect();
    routines.sort_by_key(|routine| (!routine.enabled, routine.next_due_ms));
    routines
}

/// Whether the page shows `item` among what `agent` did lately: a note or
/// question of its own, whether or not the owner has dealt with it.
fn mine(item: &Item, agent: uuid::Uuid) -> bool {
    item.agent.id == agent && item.kind != Kind::Unknown
}

/// What a note says on the page: a question in its own words.
fn said(item: &Item) -> String {
    if item.kind == Kind::Ask {
        format!("Asked: {}", item.body)
    } else {
        item.words().to_owned()
    }
}

impl Render for Profile {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let member = &self.member;
        let task_id = member.task_id;
        let now = Local::now();
        let held = match &self.loaded {
            Loaded::Ready(held) => Some(held),
            Loaded::Waiting | Loaded::Unread => None,
        };
        let line = [
            member.primary.then(|| "Your main agent".to_owned()),
            held.and_then(|held| stamp::date(held.since_ms, &now))
                .map(|day| format!("With you since {day}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let top = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(18.))
            .px(px(28.))
            .pt(px(28.))
            .pb(px(18.))
            .child(Face::new(member.look, member.primary, palette, px(72.)).alive("profile-face"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .child(member.name.clone()),
                    )
                    .when(!line.is_empty(), |words| {
                        words.child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary)
                                .child(line),
                        )
                    })
                    .when_some(self.purpose.clone(), |words, purpose| {
                        words.child(div().pt(px(4.)).line_height(px(20.)).child(purpose))
                    }),
            )
            .child(
                pill("message", "Message", true, palette)
                    .test_support()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(SheetEvent::Chat(task_id)))),
            )
            .child(
                div()
                    .id("close")
                    .absolute()
                    .top(px(14.))
                    .right(px(14.))
                    .size(px(26.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette.secondary)
                    .cursor_pointer()
                    .hover(|close| close.bg(palette.chat_agent).text_color(palette.text))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SheetEvent::Close)))
                    .child(IconName::Close),
            );
        let page = div().size_full().flex().flex_col().child(top);
        let name = &member.name;
        if self.gone {
            return page.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette.secondary)
                    .child(format!("{name} is no longer one of your agents.")),
            );
        }
        if let Some(note) = not_ready(
            &self.loaded,
            "Reading about this agent…",
            palette,
            cx.listener(|this, _, _, cx| {
                this.loaded = Loaded::Waiting;
                this.read(cx);
            }),
        ) {
            return page.child(note);
        }
        let Some(held) = held else {
            unreachable!("not_ready covers the rest")
        };
        let unread = || "Renoa couldn't read this just now.".to_owned();
        let routines: Vec<AnyElement> = held
            .routines
            .iter()
            .flatten()
            .map(|routine| {
                let when = schedule::words(&routine.schedule, &Local, self.here_zone.as_deref());
                row(
                    routine.name.clone(),
                    when,
                    if routine.enabled {
                        routine.next(&now)
                    } else {
                        "Off".to_owned()
                    },
                    None,
                    palette,
                )
                .when(!routine.enabled, |row| row.opacity(0.6))
                .into_any_element()
            })
            .collect();
        let apps: Vec<AnyElement> = held
            .apps
            .iter()
            .flatten()
            .map(|app| {
                let name = label(app);
                row(
                    name.clone(),
                    String::new(),
                    String::new(),
                    Some(mark(&name, palette)),
                    palette,
                )
                .into_any_element()
            })
            .collect();
        let lately: Vec<AnyElement> = held
            .lately
            .iter()
            .flatten()
            .map(|item| {
                row(
                    said(item),
                    String::new(),
                    stamp::short(item.at_ms, &now),
                    None,
                    palette,
                )
                .when(item.kind == Kind::Problem, |row| {
                    row.text_color(palette.danger)
                })
                .into_any_element()
            })
            .collect();
        page.child(
            div()
                .id("profile")
                .flex_1()
                .overflow_y_scroll()
                .px(px(28.))
                .pb(px(28.))
                .flex()
                .flex_col()
                .gap(px(20.))
                .child(
                    div()
                        .grid()
                        .grid_cols(2)
                        .gap(px(20.))
                        .child(column(
                            "Routines",
                            pill("all-routines", "All routines", false, palette)
                                .test_support()
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(SheetEvent::Sheet(Page::Routines))
                                })),
                            routines,
                            held.routines.as_ref().map_or_else(unread, |_| {
                                format!(
                                    "{name} runs no routines yet. Ask in the chat to set one up."
                                )
                            }),
                            palette,
                        ))
                        .child(column(
                            "Apps it uses",
                            pill("manage-apps", "Manage", false, palette)
                                .test_support()
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(SheetEvent::Sheet(Page::Apps))
                                })),
                            apps,
                            held.apps.as_ref().map_or_else(unread, |_| {
                                format!("{name} uses no apps yet. Ask in the chat to connect one.")
                            }),
                            palette,
                        )),
                )
                .child(column(
                    "Lately",
                    div(),
                    lately,
                    held.lately
                        .as_ref()
                        .map_or_else(unread, |_| format!("Nothing from {name} yet.")),
                    palette,
                )),
        )
    }
}

/// A section of the page: its title and button, then its rows, or a line
/// when it has none.
fn column(
    title: &'static str,
    action: impl IntoElement,
    rows: Vec<AnyElement>,
    empty: String,
    palette: Palette,
) -> impl IntoElement {
    let rows = if rows.is_empty() {
        div()
            .px(px(2.))
            .text_size(px(13.))
            .line_height(px(19.))
            .text_color(palette.secondary)
            .child(empty)
    } else {
        group(rows, palette)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(section(title, palette)))
                .child(action),
        )
        .child(rows)
}

/// A row: a mark, a name with a line under it, and a word at the end.
fn row(
    name: impl Into<SharedString>,
    under: String,
    end: String,
    mark: Option<gpui_kit::Div>,
    palette: Palette,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .children(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(name.into()),
                )
                .when(!under.is_empty(), |words| {
                    words.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary)
                            .truncate()
                            .child(under),
                    )
                }),
        )
        .when(!end.is_empty(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.secondary)
                    .child(end),
            )
        })
}

#[cfg(test)]
mod tests;
