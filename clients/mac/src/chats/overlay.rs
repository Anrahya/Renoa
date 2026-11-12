//! What rises over the chats: a sheet, or the ⌘K box; and the keys and the
//! sidebar's buttons that bring them up.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyView, AppContext as _, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    ease_out_quint, prelude::FluentBuilder as _,
};

use renoa_rcp_wire::TaskId;

use super::{
    Chats, SignedOut,
    jump::{Jump, JumpEvent, Place},
    keys::{self, Move},
};

use crate::{
    chat::chat_task_id,
    link::host_agent,
    sheets::{
        self, Host, Member, Page, SheetEvent, apps::AppsSheet, memory::Memory, profile::Profile,
        routines::Routines, settings::Settings,
    },
    theme::Palette,
};

use crate::theme::u;

/// How long a sheet or the jump box takes to rise into place.
const RISE: Duration = Duration::from_millis(280);
const JUMP_RISE: Duration = Duration::from_millis(180);
const SCRIM_FADE: Duration = Duration::from_millis(180);

/// What a sheet that is up shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Up {
    Sheet(Page),
    /// The page of the agent whose chat this is.
    Profile(TaskId),
}

/// A sheet that is up.
pub(super) struct Shown {
    pub(super) up: Up,
    pub(super) view: AnyView,
    /// Counts the sheets opened, so each rises afresh.
    opened: usize,
    _events: Subscription,
}

/// The ⌘K box, while it is up.
pub(super) struct Jumping {
    jump: Entity<Jump>,
    opened: usize,
    _events: Subscription,
}

impl Chats {
    /// The Host's agents as the sheets show them.
    pub(super) fn members(&self) -> Vec<Member> {
        self.agents
            .iter()
            .filter_map(|agent| {
                let host_id = host_agent(agent.target.as_str())?;
                let (look, primary) = self.face(agent);
                Some(Member {
                    name: agent.name.clone().into(),
                    host_id,
                    node_id: agent.node_id,
                    target: agent.target.clone(),
                    task_id: chat_task_id(agent.node_id, agent.target.as_str()),
                    look,
                    primary,
                })
            })
            .collect()
    }

    pub(super) fn open_sheet(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.as_ref().map(|shown| shown.up) == Some(Up::Sheet(page)) {
            return;
        }
        let host = Host(self.commands.clone());
        let members = self.members();
        let (view, events) = match page {
            Page::Routines => {
                let sheet = cx.new(|cx| Routines::new(host, members, cx));
                let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
                (AnyView::from(sheet), events)
            }
            Page::Memory => {
                let sheet = cx.new(|cx| Memory::new(host, members, cx));
                let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
                (AnyView::from(sheet), events)
            }
            Page::Apps => {
                let sheet = cx.new(|cx| AppsSheet::new(host, members, cx));
                let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
                (AnyView::from(sheet), events)
            }
            Page::Settings => {
                let sheet = cx.new(|cx| Settings::new(self.computer.clone(), cx));
                let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
                (AnyView::from(sheet), events)
            }
        };
        self.raise(Up::Sheet(page), view, events, window, cx);
    }

    /// The chat whose agent's page can open: the one showing, when its agent
    /// is one of the Host's.
    pub(super) fn profile_task(&self) -> Option<TaskId> {
        let task_id = self.chosen_task().filter(|_| !self.on_home)?;
        self.agents
            .iter()
            .any(|agent| {
                chat_task_id(agent.node_id, agent.target.as_str()) == task_id
                    && host_agent(agent.target.as_str()).is_some()
            })
            .then_some(task_id)
    }

    /// Opens the page of the agent whose chat is `task_id`, if it is one of
    /// the Host's.
    pub(super) fn open_profile(
        &mut self,
        task_id: TaskId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sheet.as_ref().map(|shown| shown.up) == Some(Up::Profile(task_id)) {
            return;
        }
        let Some(member) = self
            .members()
            .into_iter()
            .find(|member| member.task_id == task_id)
        else {
            return;
        };
        let host = Host(self.commands.clone());
        let purpose = self.purposes.get(&task_id).cloned();
        let sheet = cx.new(|cx| Profile::new(host, member, purpose, cx));
        let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
        self.raise(
            Up::Profile(task_id),
            AnyView::from(sheet),
            events,
            window,
            cx,
        );
    }

    fn raise(
        &mut self,
        up: Up,
        view: AnyView,
        events: Subscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.jumping = None;
        self.sheets_opened += 1;
        self.sheet = Some(Shown {
            up,
            view,
            opened: self.sheets_opened,
            _events: events,
        });
        // The scrim takes the keys from the composer under it, so typing
        // goes nowhere and Esc reaches the chats.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_sheet<V>(
        &mut self,
        _: &Entity<V>,
        event: &SheetEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SheetEvent::Close => self.close(window, cx),
            SheetEvent::Chat(task_id) => {
                self.close(window, cx);
                self.open_chat(*task_id, window, cx);
            }
            SheetEvent::Sheet(page) => self.open_sheet(*page, window, cx),
            SheetEvent::SignOut => cx.emit(SignedOut),
        }
    }

    /// Opens the chat `task_id`, if its agent is still listed.
    pub(super) fn open_chat(
        &mut self,
        task_id: TaskId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| chat_task_id(agent.node_id, agent.target.as_str()) == task_id)
        {
            self.open_agent(index, window, cx);
        }
    }

    /// Closes whatever is up and gives the keys back to the composer.
    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.jumping = None;
        self.refocus(window, cx);
        cx.notify();
    }

    pub(super) fn open_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        let mut places: Vec<Place> = self
            .agents
            .iter()
            .map(|agent| {
                let (look, primary) = self.face(agent);
                Place::Agent {
                    task_id: chat_task_id(agent.node_id, agent.target.as_str()),
                    name: agent.name.clone().into(),
                    look,
                    primary,
                }
            })
            .collect();
        places.insert(0, Place::Home);
        places.extend(Page::ALL.map(Place::Sheet));
        let jump = cx.new(|cx| Jump::new(places, window, cx));
        let events = cx.subscribe_in(&jump, window, |this, _, event: &JumpEvent, window, cx| {
            let JumpEvent::Go(place) = event;
            match place {
                Place::Agent { task_id, .. } => {
                    this.close(window, cx);
                    this.open_chat(*task_id, window, cx);
                }
                Place::Home => this.go_home(window, cx),
                Place::Sheet(page) => this.open_sheet(*page, window, cx),
            }
        });
        self.sheets_opened += 1;
        self.jumping = Some(Jumping {
            jump,
            opened: self.sheets_opened,
            _events: events,
        });
        cx.notify();
    }

    /// Does what a key asks; says whether it asked for anything.
    pub(super) fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(jumping) = &self.jumping
            && let key @ ("up" | "down") = event.keystroke.key.as_str()
        {
            jumping
                .jump
                .update(cx, |jump, cx| jump.step(key == "down", cx));
            return true;
        }
        keys::shortcut(&event.keystroke).is_some_and(|moved| self.go(moved, window, cx))
    }

    /// Makes `moved`; says whether it could.
    pub(super) fn go(&mut self, moved: Move, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match moved {
            Move::Close if self.sheet.is_none() && self.jumping.is_none() => return false,
            Move::Close => self.close(window, cx),
            Move::Jump => self.open_jump(window, cx),
            Move::Sheet(page) => self.open_sheet(page, window, cx),
            Move::Agent(index) => {
                if index >= self.agents.len() {
                    return false;
                }
                self.close(window, cx);
                self.open_agent(index, window, cx);
            }
            Move::Home => self.go_home(window, cx),
            Move::Profile => match self.profile_task() {
                Some(task_id) => self.open_profile(task_id, window, cx),
                None => return false,
            },
        }
        true
    }

    /// The sheet or the ⌘K box over the chats, on a scrim that closes it.
    pub(super) fn overlay(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let scrim = |opened: usize, center: bool, child: AnyElement| {
            let scrim_color = palette.scrim;
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .map(|scrim| {
                    if center {
                        scrim.items_center()
                    } else {
                        scrim.items_start().pt(u(110.))
                    }
                })
                // The chats behind see neither the pointer nor the wheel.
                .occlude()
                .track_focus(&self.focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )
                .child(child)
                .with_animation(
                    ("scrim", opened),
                    Animation::new(SCRIM_FADE).with_easing(ease_out_quint()),
                    move |scrim, t| scrim.bg(scrim_color.opacity(t)),
                )
        };
        let card = |content: AnyView| {
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content)
        };
        if let Some(jumping) = &self.jumping {
            return Some(
                scrim(
                    jumping.opened,
                    false,
                    card(jumping.jump.clone().into())
                        .relative()
                        .with_animation(
                            ("jump", jumping.opened),
                            Animation::new(JUMP_RISE).with_easing(ease_out_quint()),
                            |card, t| card.top(u(-10. * (1. - t))).opacity(t),
                        )
                        .into_any_element(),
                )
                .into_any_element(),
            );
        }
        let shown = self.sheet.as_ref()?;
        Some(
            scrim(
                shown.opened,
                true,
                card(shown.view.clone())
                    .relative()
                    .w(u(sheets::WIDTH))
                    .h(u(sheets::HEIGHT))
                    .max_h_full()
                    .rounded(u(16.))
                    .bg(palette.card)
                    .border_1()
                    .border_color(palette.outline)
                    .shadow_2xl()
                    .overflow_hidden()
                    .with_animation(
                        ("sheet", shown.opened),
                        Animation::new(RISE).with_easing(ease_out_quint()),
                        |card, t| card.top(u(18. * (1. - t))).opacity(t),
                    )
                    .into_any_element(),
            )
            .into_any_element(),
        )
    }

    /// The sidebar's foot: a button for each sheet.
    pub(super) fn sheet_buttons(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let up = self.sheet.as_ref().map(|shown| shown.up);
        div()
            .flex()
            .gap(u(2.))
            .p(u(6.))
            .border_t_1()
            .border_color(palette.outline)
            .children(Page::ALL.map(|page| {
                div()
                    .id(SharedString::from(page.title()))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(u(3.))
                    .py(u(6.))
                    .rounded(u(8.))
                    .cursor_pointer()
                    .text_color(palette.secondary)
                    .hover(|button| button.bg(palette.hover).text_color(palette.text))
                    .when(up == Some(Up::Sheet(page)), |button| {
                        button.bg(palette.selected).text_color(palette.text)
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_sheet(page, window, cx)),
                    )
                    .child(div().text_size(u(15.)).child(page.icon()))
                    .child(
                        div()
                            .text_size(u(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(page.title()),
                    )
            }))
    }

    /// The sidebar's top: the ⌘K box as a button.
    pub(super) fn jump_button(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id("jump")
            .mx(u(2.))
            .mb(u(10.))
            .h(u(32.))
            .px(u(10.))
            .flex()
            .items_center()
            .gap(u(8.))
            .rounded(u(9.))
            .bg(palette.card)
            .border_1()
            .border_color(palette.outline)
            .shadow_2xs()
            .text_color(palette.secondary)
            .cursor_pointer()
            .hover(|button| {
                button
                    .border_color(palette.outline_strong)
                    .text_color(palette.text)
            })
            .on_click(cx.listener(|this, _, window, cx| this.open_jump(window, cx)))
            .child(gpui_kit::assets::IconName::Search)
            .child(div().flex_1().child("Jump to…"))
            .child(
                div()
                    .px(u(5.))
                    .py(u(1.))
                    .rounded(u(5.))
                    .bg(palette.chat_agent)
                    .text_size(u(10.5))
                    .child("⌘K"),
            )
    }
}
