//! What the owner's agents do on their own, and when: the Host's automations,
//! each with its next run, how its last one went, a way to run it now, and a
//! switch, as the phone's Routines show them (`agents/Routines.kt`).

use std::collections::HashSet;

use chrono::Local;
use gpui_kit::{
    Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use renoa_rcp_wire::HostMethod;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    Host, Loaded, Member, Page, SheetEvent, group, header, not_ready, pill, schedule, section,
    switch,
};
use crate::{face::Face, link::host::Answer, stamp, theme::Palette};

/// The parts of the Host's observation (`GET /v1/host`) this reads.
#[derive(Debug, Deserialize)]
struct Observation {
    automations: Vec<Automation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Automation {
    id: String,
    agent_id: Uuid,
    name: String,
    schedule: schedule::Schedule,
    enabled: bool,
    revision: i64,
    next_due_ms: i64,
    #[serde(default)]
    last_run: Option<Run>,
    /// The Host plugin whose own trigger this is, such as a reminder.
    #[serde(default)]
    plugin: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Run {
    status: Outcome,
    due_ms: i64,
    #[serde(default)]
    finished_at_ms: Option<i64>,
}

/// How a run ended. The names are the Host's run statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    /// The agent finished it, even if some of its tool calls failed.
    Succeeded,
    Failed,
    /// It came due too late to be worth running.
    Skipped,
}

/// The Host's receipt for turning a routine on or off.
#[derive(Debug, Deserialize)]
struct Enablement {
    revision: i64,
    enabled: bool,
    next_due_ms: i64,
}

pub struct Routines {
    host: Host,
    members: Vec<Member>,
    loaded: Loaded<Vec<Automation>>,
    /// Routines with a change on its way.
    changing: HashSet<String>,
    /// Routines the owner just ran.
    started: HashSet<String>,
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
                this.loaded = Loaded::from_answer(answer).map(|observed| own(observed.automations));
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
                match answer.read::<Enablement>() {
                    Answer::Said(receipt) => this.took(&id, &receipt),
                    // Changed elsewhere meanwhile, or gone: read what is there now.
                    _ => {
                        this.refused =
                            Some("That didn't change. Here is how it stands now.".into());
                        this.read(cx);
                    }
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
        let body = serde_json::json!({ "operation_id": Uuid::new_v4() });
        let (host, id) = (self.host.clone(), id.to_owned());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = format!("/v1/host/automations/{id}/run");
            let answer = host
                .call(HostMethod::Post, path, Some(body.to_string()))
                .await;
            let _ = this.update(cx, |this, cx| {
                if !matches!(answer, Answer::Said(_)) {
                    this.started.remove(&id);
                    this.refused = Some("That routine didn't start. Try again in a moment.".into());
                }
                cx.notify();
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

/// The routines an agent or the owner made: a plugin's own triggers, such as
/// reminders, are the Host's work, and Memory shows them.
/// Shows the Host's receipt on routine `id`, unless what is shown is
/// already newer: a read that landed after the switch.
fn take(automations: &mut [Automation], id: &str, receipt: &Enablement) {
    if let Some(automation) = automations
        .iter_mut()
        .find(|automation| automation.id == id)
        && automation.revision < receipt.revision
    {
        automation.revision = receipt.revision;
        automation.enabled = receipt.enabled;
        automation.next_due_ms = receipt.next_due_ms;
    }
}

fn own(automations: Vec<Automation>) -> Vec<Automation> {
    automations
        .into_iter()
        .filter(|automation| automation.plugin.is_none())
        .collect()
}

/// How a routine's last run went, and whether that needs the owner.
fn last_run(run: &Run) -> (String, bool) {
    let now = Local::now();
    let at = stamp::long(run.finished_at_ms.unwrap_or(run.due_ms), &now);
    match run.status {
        Outcome::Succeeded => (format!("Last ran {}", lower_first(&at)), false),
        Outcome::Failed => (
            format!("Last run didn't finish · {}", lower_first(&at)),
            true,
        ),
        Outcome::Skipped => (
            format!("Skipped {}: it came due too late", lower_first(&at)),
            false,
        ),
    }
}

/// "Today 6:12 PM" as it reads mid-sentence: "today 6:12 PM".
fn lower_first(text: &str) -> String {
    match text.split_once(' ') {
        Some((first @ ("Today" | "Yesterday"), rest)) => format!("{} {rest}", first.to_lowercase()),
        _ => text.to_owned(),
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
            .px(px(24.))
            .pb(px(24.));
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
        let here_zone = iana_time_zone::get_timezone().ok();
        let now = Local::now();
        let row = |automation: &Automation, cx: &mut Context<Self>| {
            let member = self.member(automation.agent_id);
            let agent: SharedString =
                member.map_or_else(|| "An agent".into(), |member| member.name.clone());
            let when = schedule::words(&automation.schedule, &Local, here_zone.as_deref());
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
                .gap(px(12.))
                .px(px(14.))
                .py(px(12.))
                .when(!on, |row| row.opacity(0.6))
                .child(Face::new(
                    member.and_then(|member| member.look),
                    member.is_some_and(|member| member.primary),
                    palette,
                    px(28.),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(automation.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary)
                                .truncate()
                                .child(format!("{agent} · {when}")),
                        )
                        .when_some(last, |column, (line, trouble)| {
                            column.child(
                                div()
                                    .text_size(px(12.))
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
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.secondary)
                                    .child("Next"),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(stamp::ahead(automation.next_due_ms, &now)),
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
                .gap(px(8.))
                .when_some(self.refused.clone(), |body, refused| {
                    body.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.attention)
                            .child(refused),
                    )
                })
                .when(empty, |body| {
                    body.child(
                        div()
                            .pt(px(60.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.))
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

impl<T> Loaded<T> {
    fn map<U>(self, change: impl FnOnce(T) -> U) -> Loaded<U> {
        match self {
            Self::Waiting => Loaded::Waiting,
            Self::Ready(read) => Loaded::Ready(change(read)),
            Self::Unread => Loaded::Unread,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Enablement, Observation, Outcome, lower_first, own, take};

    const OBSERVED: &str = r#"{"automations":[
        {"id":"a1","agent_id":"1b4e28ba-2fa1-11d2-883f-0016d3cca427","name":"Dinner ideas",
         "schedule":{"kind":"cron","expression":"30 18 * * *","timezone":"Asia/Kolkata"},
         "enabled":true,"revision":3,"next_due_ms":1762695000000,
         "last_run":{"status":"failed","due_ms":1762608600000,"finished_at_ms":1762608660000}},
        {"id":"r1","agent_id":"1b4e28ba-2fa1-11d2-883f-0016d3cca427","name":"Renew the gym",
         "schedule":{"kind":"once","at":"2026-11-12T03:30:00Z"},
         "enabled":true,"revision":1,"next_due_ms":1762918200000,"plugin":"renoa.memory"}
    ],"main_agent":null}"#;

    #[test]
    fn the_hosts_automations_read_and_its_plugins_triggers_are_left_to_memory() {
        let observed: Observation =
            serde_json::from_str(OBSERVED).expect("the Host's observation reads");
        let own = own(observed.automations);
        let [dinner] = &own[..] else {
            panic!("one routine: {own:?}")
        };
        assert_eq!(dinner.name, "Dinner ideas");
        assert_eq!(
            dinner.last_run.as_ref().map(|run| run.status),
            Some(Outcome::Failed)
        );
    }

    #[test]
    fn a_day_reads_mid_sentence() {
        assert_eq!(lower_first("Today 6:12 PM"), "today 6:12 PM");
        assert_eq!(lower_first("Yesterday 9:03 AM"), "yesterday 9:03 AM");
        assert_eq!(lower_first("Monday 6:12 PM"), "Monday 6:12 PM");
    }

    #[test]
    fn a_receipt_shows_unless_a_newer_read_came_first() {
        let observed: Observation =
            serde_json::from_str(OBSERVED).expect("the Host's observation reads");
        let mut shown = own(observed.automations);
        let paused = |revision| Enablement {
            revision,
            enabled: false,
            next_due_ms: 1,
        };
        take(&mut shown, "a1", &paused(3));
        assert!(shown[0].enabled, "a receipt no newer than the read is old");
        take(&mut shown, "a1", &paused(4));
        assert!(!shown[0].enabled);
        assert_eq!((shown[0].revision, shown[0].next_due_ms), (4, 1));
        take(&mut shown, "gone", &paused(9));
        assert_eq!(shown[0].revision, 4, "a receipt for another routine");
    }
}
