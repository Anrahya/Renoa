//! What the owner's agents do on their own, and when: the Host's automations,
//! each with its next run, how its last one went, a way to run it now, and a
//! switch, as the phone's Routines show them (`agents/Routines.kt`).

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use chrono::Local;
use gpui_kit::{
    Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use renoa_rcp_wire::HostMethod;
use uuid::Uuid;

use super::{
    Host, Loaded, Member, Page, SheetEvent, group, header, not_ready, pill, schedule, section,
    switch,
};
use crate::{face::Face, link::host::Answer, theme::Palette};
use held::{Automation, Enablement, Observation, last_run, merge, own, take};

use crate::theme::u;

pub(crate) mod held;

/// How long a routine the owner ran reads Started, before the sheet reads
/// the routines again to show how the run went.
const STARTED_FOR: Duration = Duration::from_secs(5);

pub struct Routines {
    host: Host,
    members: Vec<Member>,
    loaded: Loaded<Vec<Automation>>,
    /// Routines with a change on its way.
    changing: HashSet<String>,
    /// Routines the owner just ran.
    started: HashSet<String>,
    /// The run asked of each routine that the Host has not confirmed: asked
    /// again, it is the same run.
    runs: HashMap<String, Uuid>,
    /// This Mac's timezone, which a schedule on another clock is named
    /// against.
    here_zone: Option<String>,
    /// A change the Host refused, said once.
    refused: Option<SharedString>,
}

impl EventEmitter<SheetEvent> for Routines {}

impl Routines {
    pub fn new(host: Host, members: Vec<Member>, cx: &mut Context<Self>) -> Self {
        let mut routines = Self {
            host,
            members,
            loaded: Loaded::Waiting,
            changing: HashSet::new(),
            started: HashSet::new(),
            runs: HashMap::new(),
            here_zone: iana_time_zone::get_timezone().ok(),
            refused: None,
        };
        routines.read(cx);
        routines
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let answer = host.get::<Observation>("/v1/host".to_owned()).await;
            let _ = this.update(cx, |this, cx| {
                let shown = match &this.loaded {
                    Loaded::Ready(shown) => Some(shown.as_slice()),
                    Loaded::Waiting | Loaded::Unread => None,
                };
                match (Loaded::from_answer(answer), shown.is_some()) {
                    // What is shown stays, with a word that it may be behind.
                    (Loaded::Unread, true) => {
                        this.refused = Some("Renoa couldn't be reached just now.".into());
                    }
                    (read, _) => {
                        let read = read.map(|observed| merge(shown, own(observed.automations)));
                        this.loaded = read;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        let Loaded::Ready(automations) = &self.loaded else {
            return;
        };
        let Some(automation) = automations.iter().find(|automation| automation.id == id) else {
            return;
        };
        if automation.enabled == on || !self.changing.insert(id.to_owned()) {
            return;
        }
        let body = serde_json::json!({
            "operation_id": Uuid::new_v4(),
            "expected_revision": automation.revision,
            "enabled": on,
        });
        let (host, id) = (self.host.clone(), id.to_owned());
        self.refused = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = format!("/v1/host/automations/{id}/enabled");
            let answer = host
                .call(HostMethod::Post, path, Some(body.to_string()))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.changing.remove(&id);
                let refused = match answer.read::<Enablement>() {
                    Answer::Said(receipt) => {
                        this.took(&id, &receipt);
                        None
                    }
                    Answer::Conflict => Some("It changed meanwhile. Here is how it stands now."),
                    Answer::Missing => Some("That routine is gone."),
                    Answer::Later | Answer::Lost => {
                        Some("Renoa couldn't confirm that. Here is how it stands now.")
                    }
                };
                if let Some(refused) = refused {
                    this.refused = Some(refused.into());
                    this.read(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps the Host's receipt, unless a read meanwhile saw a newer revision.
    fn took(&mut self, id: &str, receipt: &Enablement) {
        if let Loaded::Ready(automations) = &mut self.loaded {
            take(automations, id, receipt);
        }
    }

    fn run_now(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.started.contains(id) {
            return;
        }
        self.started.insert(id.to_owned());
        self.refused = None;
        let operation_id = *self.runs.entry(id.to_owned()).or_insert_with(Uuid::new_v4);
        let body = serde_json::json!({ "operation_id": operation_id });
        let (host, id) = (self.host.clone(), id.to_owned());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = format!("/v1/host/automations/{id}/run");
            let answer = host
                .call(HostMethod::Post, path, Some(body.to_string()))
                .await;
            let ran = this.update(cx, |this, cx| {
                let refused = match answer {
                    // Started, or a run of it already under way.
                    Answer::Said(_) | Answer::Conflict => None,
                    Answer::Missing => Some("That routine is gone."),
                    Answer::Later | Answer::Lost => {
                        Some("Renoa couldn't confirm that routine started. Try again in a moment.")
                    }
                };
                if !matches!(answer, Answer::Later | Answer::Lost) {
                    this.runs.remove(&id);
                }
                cx.notify();
                let Some(refused) = refused else {
                    return true;
                };
                this.started.remove(&id);
                this.refused = Some(refused.into());
                false
            });
            if !matches!(ran, Ok(true)) {
                return;
            }
            cx.background_executor().timer(STARTED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.started.remove(&id);
                this.read(cx);
            });
        })
        .detach();
    }

    fn member(&self, agent_id: Uuid) -> Option<&Member> {
        self.members
            .iter()
            .find(|member| member.host_id == agent_id)
    }
}

impl Render for Routines {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let main = self.members.iter().find(|member| member.primary).cloned();
        let ask = main.map(|main| {
            pill("new", "New routine", true, palette)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(SheetEvent::Chat(main.task_id))))
                .into_any_element()
        });
        let top = header(
            Page::Routines,
            "What your agents do on their own, and when.",
            palette,
            ask.into_iter().collect(),
            cx.listener(|_, _, _, cx| cx.emit(SheetEvent::Close)),
        );
        let body = div()
            .id("routines")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(u(24.))
            .pb(u(24.));
        if let Some(note) = not_ready(
            &self.loaded,
            "Reading your routines…",
            palette,
            cx.listener(|this, _, _, cx| {
                this.loaded = Loaded::Waiting;
                this.read(cx);
            }),
        ) {
            return div().size_full().flex().flex_col().child(top).child(note);
        }
        let Loaded::Ready(automations) = &self.loaded else {
            unreachable!("not_ready covers the rest")
        };
        let now = Local::now();
        let row = |automation: &Automation, cx: &mut Context<Self>| {
            let member = self.member(automation.agent_id);
            let agent: SharedString =
                member.map_or_else(|| "An agent".into(), |member| member.name.clone());
            let when = schedule::words(&automation.schedule, &Local, self.here_zone.as_deref());
            let last = automation.last_run.as_ref().map(last_run);
            let id = automation.id.clone();
            let (on, busy, started) = (
                automation.enabled,
                self.changing.contains(&id),
                self.started.contains(&id),
            );
            div()
                .flex()
                .items_center()
                .gap(u(12.))
                .px(u(14.))
                .py(u(12.))
                .when(!on, |row| row.opacity(0.6))
                .child(Face::new(
                    member.and_then(|member| member.look),
                    member.is_some_and(|member| member.primary),
                    palette,
                    u(28.),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(u(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(automation.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(u(12.))
                                .text_color(palette.secondary)
                                .truncate()
                                .child(format!("{agent} · {when}")),
                        )
                        .when_some(last, |column, (line, trouble)| {
                            column.child(
                                div()
                                    .text_size(u(12.))
                                    .truncate()
                                    .text_color(if trouble {
                                        palette.danger
                                    } else {
                                        palette.secondary
                                    })
                                    .child(line),
                            )
                        }),
                )
                .when(on, |row| {
                    row.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(u(2.))
                            .child(
                                div()
                                    .text_size(u(11.))
                                    .text_color(palette.secondary)
                                    .child("Next"),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(automation.next(&now)),
                            ),
                    )
                })
                .child({
                    let id = id.clone();
                    pill(
                        SharedString::from(format!("run-{}", automation.id)),
                        if started { "Started" } else { "Run now" },
                        false,
                        palette,
                    )
                    .when(started, |pill| pill.opacity(0.6))
                    .on_click(cx.listener(move |this, _, _, cx| this.run_now(&id, cx)))
                })
                .child(
                    switch(
                        SharedString::from(format!("on-{}", automation.id)),
                        on,
                        palette,
                    )
                    .when(busy, |switch| switch.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.set(&id, !on, cx))),
                )
                .into_any_element()
        };
        let (on, paused): (Vec<&Automation>, Vec<&Automation>) = automations
            .iter()
            .partition(|automation| automation.enabled);
        let on_rows: Vec<_> = on.iter().map(|automation| row(automation, cx)).collect();
        let paused_rows: Vec<_> = paused
            .iter()
            .map(|automation| row(automation, cx))
            .collect();
        let empty = automations.is_empty();
        div().size_full().flex().flex_col().child(top).child(
            body.flex()
                .flex_col()
                .gap(u(8.))
                .when_some(self.refused.clone(), |body, refused| {
                    body.child(
                        div()
                            .text_size(u(12.))
                            .text_color(palette.attention)
                            .child(refused),
                    )
                })
                .when(empty, |body| {
                    body.child(
                        div()
                            .pt(u(60.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(u(6.))
                            .text_color(palette.secondary)
                            .child(
                                div()
                                    .text_color(palette.text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("No routines yet"),
                            )
                            .child("Ask an agent to do something on a schedule,")
                            .child("like “every morning at 8, tell me the weather”."),
                    )
                })
                .when(!on_rows.is_empty(), |body| {
                    body.child(section("On", palette))
                        .child(group(on_rows, palette))
                })
                .when(!paused_rows.is_empty(), |body| {
                    body.child(section("Paused", palette))
                        .child(group(paused_rows, palette))
                }),
        )
    }
}
