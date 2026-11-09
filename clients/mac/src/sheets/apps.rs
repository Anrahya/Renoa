//! The apps connected to Renoa, and which agents use each: a switch per
//! agent, as the phone's Apps page has one for the main agent
//! (`home/AppsPage.kt`). Connecting a new app happens in a chat.

use std::collections::{BTreeMap, HashSet};

use gpui_kit::{
    Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use renoa_rcp_wire::HostMethod;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    Host, Loaded, Member, Page, SheetEvent, group, header, mark, not_ready, pill, schedule,
};
use crate::{face::Face, link::host::Answer, theme::Palette};

/// An app an agent can use: a connection on the Host, named by the plugin
/// that serves it, if any. Switching it off keeps its sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct AgentApp {
    pub(super) connection: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    endpoint: String,
    pub(super) on: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct Apps {
    pub(super) apps: Vec<AgentApp>,
}

/// Each agent's apps, by the agent's Host id.
type Held = BTreeMap<Uuid, Vec<AgentApp>>;

pub struct AppsSheet {
    host: Host,
    members: Vec<Member>,
    loaded: Loaded<Held>,
    /// Switches on their way, by agent and connection.
    changing: HashSet<(Uuid, String)>,
    refused: bool,
}

impl EventEmitter<SheetEvent> for AppsSheet {}

impl AppsSheet {
    pub fn new(host: Host, members: Vec<Member>, cx: &mut Context<Self>) -> Self {
        let mut apps = Self {
            host,
            members,
            loaded: Loaded::Waiting,
            changing: HashSet::new(),
            refused: false,
        };
        apps.read(cx);
        apps
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let agents: Vec<Uuid> = self.members.iter().map(|member| member.host_id).collect();
        cx.spawn(async move |this, cx| {
            let mut held = Held::new();
            let mut unread = false;
            for agent in agents {
                match host
                    .get::<Apps>(format!("/v1/host/agents/{agent}/apps"))
                    .await
                {
                    Answer::Said(apps) => {
                        held.insert(agent, apps.apps);
                    }
                    // An agent gone meanwhile has no apps to show.
                    Answer::Missing => {}
                    Answer::Conflict | Answer::Later | Answer::Lost => unread = true,
                }
            }
            let _ = this.update(cx, |this, cx| {
                match (unread, &this.loaded) {
                    // What is shown stays; the note above it says it may be behind.
                    (true, Loaded::Ready(_)) => this.refused = true,
                    (true, _) => this.loaded = Loaded::Unread,
                    (false, _) => this.loaded = Loaded::Ready(held),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set(&mut self, agent: Uuid, connection: &str, on: bool, cx: &mut Context<Self>) {
        if !self.changing.insert((agent, connection.to_owned())) {
            return;
        }
        self.refused = false;
        let host = self.host.clone();
        let connection = connection.to_owned();
        let body = serde_json::json!({ "enabled": on }).to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = format!(
                "/v1/host/agents/{agent}/apps/{}/enabled",
                segment(&connection)
            );
            let answer = host
                .call(HostMethod::Post, path, Some(body))
                .await
                .read::<Apps>();
            let _ = this.update(cx, |this, cx| {
                this.changing.remove(&(agent, connection.clone()));
                match answer {
                    Answer::Said(apps) => {
                        if let Loaded::Ready(held) = &mut this.loaded {
                            adopt(held, agent, &connection, &apps.apps);
                        }
                    }
                    // Refused, or unconfirmed: read how things stand.
                    _ => {
                        this.refused = true;
                        this.read(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// `text` as one path segment: anything but a letter, a digit or `-._~` is
/// percent-encoded, as the phone does (`encodedSegment`).
fn segment(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// What an app is called, in plain words: its plugin's name, else its
/// connection's, without the word mcp and with spaces for dashes, such as
/// Exa for exa-mcp.
pub(super) fn label(app: &AgentApp) -> String {
    let words: Vec<&str> = app
        .name
        .as_deref()
        .unwrap_or(&app.connection)
        .split(['-', '_', '.', ' '])
        .filter(|word| !word.is_empty() && !word.eq_ignore_ascii_case("mcp"))
        .collect();
    let joined = words.join(" ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => app.connection.clone(),
    }
}

/// Where an app lives, as a person reads an address: its host name.
fn site(endpoint: &str) -> String {
    let rest = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    rest.split(['/', ':'])
        .next()
        .unwrap_or(rest)
        .trim_start_matches("www.")
        .to_owned()
}

/// Takes from the Host's answer to one switch only that switch: answers to
/// two switches of one agent can come back in either order.
fn adopt(held: &mut Held, agent: Uuid, connection: &str, answered: &[AgentApp]) {
    let Some(on) = answered
        .iter()
        .find(|app| app.connection == connection)
        .map(|app| app.on)
    else {
        return;
    };
    if let Some(app) = held
        .get_mut(&agent)
        .into_iter()
        .flatten()
        .find(|app| app.connection == connection)
    {
        app.on = on;
    }
}

/// Every connection any agent has, once each, in name order, with each
/// agent's switch for it.
fn connections(held: &Held) -> Vec<(AgentApp, Vec<(Uuid, bool)>)> {
    let mut by_connection: BTreeMap<String, (AgentApp, Vec<(Uuid, bool)>)> = BTreeMap::new();
    for (agent, apps) in held {
        for app in apps {
            by_connection
                .entry(app.connection.clone())
                .or_insert_with(|| (app.clone(), Vec::new()))
                .1
                .push((*agent, app.on));
        }
    }
    let mut listed: Vec<_> = by_connection.into_values().collect();
    listed.sort_by_key(|(app, _)| label(app).to_lowercase());
    listed
}

impl Render for AppsSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let main = self.members.iter().find(|member| member.primary).cloned();
        let top = header(
            Page::Apps,
            "The apps your agents can use for you. Each agent only gets the ones switched on for it.",
            palette,
            Vec::new(),
            cx.listener(|_, _, _, cx| cx.emit(SheetEvent::Close)),
        );
        if let Some(note) = not_ready(
            &self.loaded,
            "Checking which apps are connected…",
            palette,
            cx.listener(|this, _, _, cx| {
                this.loaded = Loaded::Waiting;
                this.read(cx);
            }),
        ) {
            return div().size_full().flex().flex_col().child(top).child(note);
        }
        let Loaded::Ready(held) = &self.loaded else {
            unreachable!("not_ready covers the rest")
        };
        let listed = connections(held);
        let rows: Vec<_> = listed
            .into_iter()
            .map(|(app, switches)| {
                let users: Vec<&str> = switches
                    .iter()
                    .filter(|(_, on)| *on)
                    .filter_map(|(agent, _)| {
                        self.members.iter().find(|member| member.host_id == *agent)
                    })
                    .map(|member| member.name.as_ref())
                    .collect();
                let line = if users.is_empty() {
                    format!("{} · Off for every agent", site(&app.endpoint))
                } else {
                    format!("{} · {}", site(&app.endpoint), schedule::listed(&users))
                };
                let chips = switches.into_iter().filter_map(|(agent, on)| {
                    let member = self
                        .members
                        .iter()
                        .find(|member| member.host_id == agent)?
                        .clone();
                    let connection = app.connection.clone();
                    let busy = self.changing.contains(&(agent, connection.clone()));
                    Some(
                        div()
                            .id(SharedString::from(format!("{agent}-{connection}")))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pl(px(3.))
                            .pr(px(10.))
                            .py(px(3.))
                            .rounded_full()
                            .border_1()
                            .cursor_pointer()
                            .map(|chip| {
                                if on {
                                    chip.bg(palette.text)
                                        .text_color(palette.background)
                                        .border_color(palette.text)
                                } else {
                                    chip.border_color(palette.outline)
                                        .text_color(palette.secondary)
                                }
                            })
                            .when(busy, |chip| chip.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set(agent, &connection, !on, cx)
                            }))
                            .child(Face::new(member.look, member.primary, palette, px(18.)))
                            .child(div().text_size(px(12.)).child(member.name.clone())),
                    )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(mark(&label(&app), palette))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child(label(&app)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(palette.secondary)
                                            .truncate()
                                            .child(line),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(42.))
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(chips),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div().size_full().flex().flex_col().child(top).child(
            div()
                .id("apps")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(24.))
                .pb(px(24.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .when(self.refused, |body| {
                    body.child(div().text_size(px(12.)).text_color(palette.attention).child("Renoa couldn't confirm that. Here is how it stands now."))
                })
                .when(empty, |body| {
                    body.child(div().pt(px(40.)).flex().justify_center().text_color(palette.secondary).child(if self.members.is_empty() {
                        "Your agents aren't online just now."
                    } else {
                        "No apps are connected yet."
                    }))
                })
                .when(!empty, |body| {
                    body.child(group(rows, palette)).child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary)
                            .child("Turning an app off for an agent doesn't disconnect it. Turn it back on any time."),
                    )
                })
                .when_some(main, |body, main| {
                    let task_id = main.task_id;
                    body.child(
                        div()
                            .pt(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().flex_1().text_color(palette.secondary).child(format!("To connect another app, ask {}.", main.name)))
                            .child(
                                pill("ask", format!("Ask {}", main.name), true, palette)
                                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(SheetEvent::Chat(task_id)))),
                            ),
                    )
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::{AgentApp, adopt, connections, label, segment, site};

    fn app(connection: &str, name: Option<&str>, on: bool) -> AgentApp {
        AgentApp {
            connection: connection.to_owned(),
            name: name.map(str::to_owned),
            endpoint: "https://mcp.exa.ai/mcp".to_owned(),
            on,
        }
    }

    #[test]
    fn an_app_reads_by_its_name_and_its_site() {
        assert_eq!(label(&app("exa-mcp", None, true)), "Exa");
        assert_eq!(
            label(&app("x", Some("google_calendar"), true)),
            "Google calendar"
        );
        assert_eq!(label(&app("mcp", None, true)), "mcp");
        assert_eq!(site("https://www.notion.so/mcp/sse"), "notion.so");
        assert_eq!(site("http://127.0.0.1:8080/x"), "127.0.0.1");
    }

    #[test]
    fn each_connection_shows_once_with_every_agents_switch() {
        let (chef, scout) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let held = BTreeMap::from([
            (
                chef,
                vec![
                    app("exa-mcp", None, true),
                    app("cal", Some("calendar"), false),
                ],
            ),
            (scout, vec![app("exa-mcp", None, false)]),
        ]);
        let listed = connections(&held);
        let names: Vec<String> = listed.iter().map(|(app, _)| label(app)).collect();
        assert_eq!(names, ["Calendar", "Exa"]);
        assert_eq!(listed[1].1, [(chef, true), (scout, false)]);
    }

    #[test]
    fn a_connection_stays_one_path_segment() {
        assert_eq!(segment("exa-mcp"), "exa-mcp");
        assert_eq!(segment("a/../b c"), "a%2F..%2Fb%20c");
    }

    #[test]
    fn an_answer_to_one_switch_changes_only_that_switch() {
        let agent = Uuid::from_u128(1);
        let mut held = BTreeMap::from([(
            agent,
            vec![app("github", None, false), app("notion", None, false)],
        )]);
        // The answer to the github switch, sent before the notion switch
        // was answered, says notion is still off.
        adopt(
            &mut held,
            agent,
            "github",
            &[app("github", None, true), app("notion", None, false)],
        );
        held.get_mut(&agent).expect("held")[1].on = true;
        adopt(
            &mut held,
            agent,
            "github",
            &[app("github", None, true), app("notion", None, false)],
        );
        assert_eq!(
            held[&agent].iter().map(|app| app.on).collect::<Vec<_>>(),
            [true, true]
        );
        adopt(
            &mut held,
            Uuid::from_u128(2),
            "github",
            &[app("github", None, false)],
        );
        assert!(held[&agent][0].on, "another agent's answer");
    }
}
