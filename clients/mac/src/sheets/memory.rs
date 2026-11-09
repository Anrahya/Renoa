//! What the agents know about the owner: the profile they all read, what
//! they will bring up and when, and the newest notes they keep, as the
//! phone's Memory page shows them (`home/MemoryPage.kt`). Read only: the
//! owner changes it by telling an agent.

use chrono::{NaiveDate, NaiveDateTime};
use gpui_kit::{
    Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, assets::IconName,
    component::text::TextView, div, prelude::FluentBuilder as _, px,
};
use serde::Deserialize;

use super::{Host, Loaded, Member, Page, SheetEvent, group, header, not_ready, pill, section};
use crate::{face::Face, link::host::Answer, stamp, theme::Palette};

/// The owner's profile: the `USER.md` every agent that talks to them reads.
#[derive(Debug, Clone, Deserialize)]
struct Profile {
    content: String,
}

/// A note an agent keeps about the owner, in its own words. `written` is
/// the day, `YYYY-MM-DD`; `happened` the day it says something happened,
/// when that differs; `due` when it asks to be brought up,
/// `YYYY-MM-DDTHH:MM` in the owner's zone.
#[derive(Debug, Clone, Deserialize)]
struct Note {
    written: String,
    #[serde(default)]
    author: Option<String>,
    text: String,
    #[serde(default)]
    happened: Option<String>,
    #[serde(default)]
    due: Option<String>,
}

/// The newest notes, newest first, and how many there are in all.
#[derive(Debug, Clone, Deserialize)]
struct Notes {
    notes: Vec<Note>,
    total: u64,
}

#[derive(Debug, Clone)]
struct Known {
    profile: String,
    notes: Notes,
}

pub struct Memory {
    host: Host,
    members: Vec<Member>,
    loaded: Loaded<Known>,
}

impl EventEmitter<SheetEvent> for Memory {}

impl Memory {
    pub fn new(host: Host, members: Vec<Member>, cx: &mut Context<Self>) -> Self {
        let mut memory = Self {
            host,
            members,
            loaded: Loaded::Waiting,
        };
        memory.read(cx);
        memory
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let host = self.host.clone();
        cx.spawn(async move |this, cx| {
            let profile = host.get::<Profile>("/v1/host/profile".to_owned()).await;
            let notes = host.get::<Notes>("/v1/host/memory".to_owned()).await;
            let loaded = match (profile, notes) {
                (Answer::Said(profile), Answer::Said(notes)) => Loaded::Ready(Known {
                    profile: profile.content,
                    notes,
                }),
                _ => Loaded::Unread,
            };
            let _ = this.update(cx, |this, cx| {
                this.loaded = loaded;
                cx.notify();
            });
        })
        .detach();
    }
}

/// A day as "27 Oct", or nothing for one that does not read.
fn day(date: &str) -> Option<String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|day| day.format("%-d %b").to_string())
}

/// A reminder's time as "12 Nov, 9 AM", or nothing for one that does not read.
fn due(at: &str) -> Option<String> {
    let at = NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M").ok()?;
    Some(format!(
        "{}, {}",
        at.format("%-d %b"),
        stamp::clock(at.time())
    ))
}

impl Render for Memory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let main = self.members.iter().find(|member| member.primary).cloned();
        let tell = main.as_ref().map(|main| {
            let task_id = main.task_id;
            pill("tell", format!("Tell {}", main.name), false, palette)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(SheetEvent::Chat(task_id))))
                .into_any_element()
        });
        let top = header(
            Page::Memory,
            "What your agents know about you. To change something, tell them in a chat.",
            palette,
            tell.into_iter().collect(),
            cx.listener(|_, _, _, cx| cx.emit(SheetEvent::Close)),
        );
        if let Some(note) = not_ready(
            &self.loaded,
            "Reading what your agents remember…",
            palette,
            cx.listener(|this, _, _, cx| {
                this.loaded = Loaded::Waiting;
                this.read(cx);
            }),
        ) {
            return div().size_full().flex().flex_col().child(top).child(note);
        }
        let Loaded::Ready(known) = &self.loaded else {
            unreachable!("not_ready covers the rest")
        };
        let about = known.profile.trim().to_owned();
        let reminders: Vec<(String, String)> = known
            .notes
            .notes
            .iter()
            .filter_map(|note| Some((note.text.clone(), due(note.due.as_deref()?)?)))
            .collect();
        let notes: Vec<_> = known
            .notes
            .notes
            .iter()
            .filter(|note| note.due.is_none())
            .map(|note| {
                let member = note.author.as_deref().and_then(|author| {
                    self.members
                        .iter()
                        .find(|member| member.name.as_ref() == author)
                });
                let by = [
                    note.author.clone(),
                    day(note.happened.as_deref().unwrap_or(&note.written)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                div()
                    .flex()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(palette.divider)
                    .child(match member {
                        Some(member) => Face::new(member.look, member.primary, palette, px(22.))
                            .into_any_element(),
                        // A note no agent signed, such as one the owner wrote.
                        None => div()
                            .size(px(22.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(palette.secondary)
                            .child(IconName::BookOpen)
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(note.text.clone())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.secondary)
                                    .child(by),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let hidden = known.notes.total > known.notes.notes.len() as u64;
        if about.is_empty() && known.notes.notes.is_empty() {
            return div().size_full().flex().flex_col().child(top).child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .text_color(palette.secondary)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text)
                            .child("Nothing yet"),
                    )
                    .child("Tell an agent about yourself, and what it keeps shows here."),
            );
        }
        let left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(section("About you", palette))
            .child(
                div()
                    .px(px(16.))
                    .py(px(14.))
                    .rounded(px(14.))
                    .bg(palette.group)
                    .map(|card| {
                        if about.is_empty() {
                            card.text_color(palette.secondary).child(
                                "Nothing yet. What you tell your agents about yourself goes here.",
                            )
                        } else {
                            card.child(TextView::markdown("about", SharedString::from(about)))
                        }
                    }),
            )
            .when(!reminders.is_empty(), |left| {
                left.child(section("Reminders", palette)).child(group(
                    reminders.into_iter().map(|(text, at)| {
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(14.))
                            .py(px(10.))
                            .child(div().min_w_0().child(text))
                            .child(div().flex_none().text_color(palette.secondary).child(at))
                            .into_any_element()
                    }),
                    palette,
                ))
            });
        let right = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(section("Newest notes", palette))
            .children(notes)
            .when(hidden, |right| {
                right.child(
                    div()
                        .pt(px(4.))
                        .text_size(px(12.))
                        .text_color(palette.secondary)
                        .child("Older notes aren't shown here. Ask about them in a chat."),
                )
            });
        div().size_full().flex().flex_col().child(top).child(
            div()
                .id("memory")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(24.))
                .pb(px(24.))
                .flex()
                .gap(px(20.))
                .child(left)
                .child(right),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Notes, day, due};

    #[test]
    fn the_hosts_notes_read_and_their_days_read_as_people_write_them() {
        let notes: Notes = serde_json::from_str(
            r#"{"notes":[{"id":7,"written":"2026-11-09","author":"Chef","text":"Less spicy next time."},
                         {"id":6,"written":"2026-11-08","text":"Gym renewal","due":"2026-11-12T09:00"}],"total":9}"#,
        )
        .expect("the Host's notes read");
        assert_eq!(notes.notes.len(), 2);
        assert_eq!(day(&notes.notes[0].written).as_deref(), Some("9 Nov"));
        assert_eq!(due("2026-11-12T09:00").as_deref(), Some("12 Nov, 9 AM"));
        assert_eq!(due("soon"), None);
    }
}
