//! The sheets that rise over the chats: Routines, Memory, Apps and Settings.
//! Each reads what it shows from the owner's Host through the session, and
//! closes back to the chat the owner had open.

use gpui_kit::{
    AnimationExt as _, AnyElement, App, ClickEvent, Div, ElementId, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString, SpringAnimation,
    SpringConfig, Stateful, StatefulInteractiveElement as _, Styled as _, Window, assets::IconName,
    div, prelude::FluentBuilder as _,
};
use renoa_protocol::TargetRef;
use renoa_rcp_wire::{HostMethod, NodeId, TaskId};
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    face::Look,
    link::{
        Command,
        host::{Answer, HostCall},
    },
    theme::Palette,
};

use crate::theme::u;

pub mod apps;
pub mod memory;
pub mod profile;
pub mod routines;
pub mod schedule;
pub mod settings;

/// Which sheet is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Routines,
    Memory,
    Apps,
    Settings,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Routines, Self::Memory, Self::Apps, Self::Settings];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Routines => "Routines",
            Self::Memory => "Memory",
            Self::Apps => "Apps",
            Self::Settings => "Settings",
        }
    }

    pub const fn icon(self) -> IconName {
        match self {
            Self::Routines => IconName::Calendar,
            Self::Memory => IconName::BookOpen,
            Self::Apps => IconName::LayoutDashboard,
            Self::Settings => IconName::Settings,
        }
    }

    /// The keys that open it, as the menu and the sheet show them.
    pub const fn keys(self) -> &'static str {
        match self {
            Self::Routines => "⌘⇧R",
            Self::Memory => "⌘⇧M",
            Self::Apps => "⌘⇧A",
            Self::Settings => "⌘,",
        }
    }
}

/// What a sheet asks of the chats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetEvent {
    Close,
    /// Opens the chat with this agent, as "Tell Renoa" does.
    Chat(TaskId),
    /// Opens this sheet in its place.
    Sheet(Page),
    SignOut,
}

/// An agent as the sheets show it: one of the Host's, with its chat and face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: SharedString,
    pub host_id: Uuid,
    /// Where its chat runs, to open it before writing to it.
    pub node_id: NodeId,
    pub target: TargetRef,
    pub task_id: TaskId,
    pub look: Option<Look>,
    pub primary: bool,
}

/// The owner's Host, as the views reach it through the session.
#[derive(Clone)]
pub struct Host(pub mpsc::UnboundedSender<Command>);

impl Host {
    pub async fn call(
        &self,
        method: HostMethod,
        path: String,
        body: Option<String>,
    ) -> Answer<String> {
        let (answered, answer) = oneshot::channel();
        let call = HostCall {
            method,
            path,
            body,
            answered,
        };
        if self.0.send(Command::Host(call)).is_err() {
            return Answer::Lost;
        }
        answer.await.unwrap_or(Answer::Lost)
    }

    pub async fn get<T: DeserializeOwned>(&self, path: String) -> Answer<T> {
        self.call(HostMethod::Get, path, None).await.read()
    }
}

/// What a sheet shows while it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded<T> {
    Waiting,
    Ready(T),
    /// Renoa could not be reached, or the Host could not answer.
    Unread,
}

impl<T> Loaded<T> {
    pub fn from_answer(answer: Answer<T>) -> Self {
        match answer {
            Answer::Said(read) => Self::Ready(read),
            Answer::Missing | Answer::Conflict | Answer::Later | Answer::Lost => Self::Unread,
        }
    }

    pub fn map<U>(self, change: impl FnOnce(T) -> U) -> Loaded<U> {
        match self {
            Self::Waiting => Loaded::Waiting,
            Self::Ready(read) => Loaded::Ready(change(read)),
            Self::Unread => Loaded::Unread,
        }
    }
}

/// The width and height of a sheet.
pub const WIDTH: f32 = 800.;
pub const HEIGHT: f32 = 620.;

/// A sheet's top: its title, a line on what it is for, its own actions and
/// the close button.
pub fn header(
    page: Page,
    line: impl Into<SharedString>,
    palette: Palette,
    actions: Vec<AnyElement>,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(u(12.))
        .px(u(24.))
        .pt(u(20.))
        .pb(u(14.))
        .border_b_1()
        .border_color(palette.divider)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(u(2.))
                .child(
                    div()
                        .text_size(u(17.))
                        .font_weight(FontWeight::BOLD)
                        .child(page.title()),
                )
                .child(
                    div()
                        .text_size(u(12.))
                        .text_color(palette.secondary)
                        .child(line.into()),
                ),
        )
        .children(actions)
        .child(
            div()
                .id("close")
                .size(u(30.))
                .flex_none()
                .rounded_full()
                .bg(palette.chat_agent)
                .text_color(palette.secondary)
                .text_size(u(14.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|close| close.bg(palette.selected).text_color(palette.text))
                .on_click(close)
                .child(IconName::Close),
        )
}

/// The small capitals over a group of rows.
pub fn section(title: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .flex_none()
        .pt(u(6.))
        .pb(u(2.))
        .text_size(u(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.secondary)
        .child(title.into().to_uppercase())
}

/// A group of rows on a sheet: they sit together on a soft ground, with a
/// line between each.
pub fn group(rows: impl IntoIterator<Item = AnyElement>, palette: Palette) -> Div {
    let rows: Vec<AnyElement> = rows.into_iter().collect();
    let last = rows.len().saturating_sub(1);
    div()
        .flex_none()
        .rounded(u(14.))
        .bg(palette.card)
        .border_1()
        .border_color(palette.outline)
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(rows.into_iter().enumerate().map(move |(index, row)| {
            div()
                .when(index < last, |row| {
                    row.border_b_1().border_color(palette.divider)
                })
                .child(row)
                .into_any_element()
        }))
}

/// A rounded button: ink-filled when it is the sheet's main action, outlined
/// when not.
pub fn pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    filled: bool,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(u(28.))
        .px(u(12.))
        .flex_none()
        .flex()
        .items_center()
        .rounded_full()
        .text_size(u(12.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|pill| {
            if filled {
                pill.bg(palette.text)
                    .text_color(palette.background)
                    .hover(|pill| pill.opacity(0.86))
            } else {
                pill.border_1()
                    .border_color(palette.outline)
                    .bg(palette.card)
                    .hover(|pill| pill.bg(palette.hover).border_color(palette.outline_strong))
            }
        })
        .child(label.into())
}

/// How the switch's knob moves: quick, with a little give.
const SWITCH: SpringConfig = SpringConfig::new(520., 30., 1.);

/// An on and off switch; its knob slides when it changes.
pub fn switch(id: impl Into<ElementId>, on: bool, palette: Palette) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .w(u(34.))
        .h(u(20.))
        .flex_none()
        .relative()
        .rounded_full()
        .cursor_pointer()
        .bg(if on { palette.text } else { palette.outline })
        .child(
            div()
                .absolute()
                .top(u(2.))
                .size(u(16.))
                .rounded_full()
                .bg(palette.background)
                .with_spring(
                    id,
                    SpringAnimation::new(SWITCH).to(u(if on { 16. } else { 2. })),
                    |knob, left| knob.left(left),
                ),
        )
}

/// What a sheet says while it reads, or when it could not.
pub fn not_ready<T>(
    loaded: &Loaded<T>,
    what: &'static str,
    palette: Palette,
    again: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Option<Div> {
    let note = |line: &'static str| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(u(10.))
            .text_color(palette.secondary)
            .child(line)
    };
    match loaded {
        Loaded::Ready(_) => None,
        Loaded::Waiting => Some(note(what)),
        Loaded::Unread => Some(
            note("Renoa couldn't be reached just now.")
                .child(pill("again", "Try again", false, palette).on_click(again)),
        ),
    }
}

/// A tile with the first letter of an app's name, where the app has no mark
/// of its own.
pub fn mark(name: &str, palette: Palette) -> Div {
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "·".to_owned(), |c| c.to_uppercase().collect());
    div()
        .size(u(30.))
        .flex_none()
        .rounded(u(8.))
        .bg(palette.chat_agent)
        .border_1()
        .border_color(palette.outline)
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::BOLD)
        .child(letter)
}
