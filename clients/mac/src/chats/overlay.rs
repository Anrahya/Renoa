//! What rises over the chats: a sheet, or the ⌘K box; and the keys and the
//! sidebar's buttons that bring them up.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, AnyView, AppContext as _, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    ease_out_quint, prelude::FluentBuilder as _, px,
};

use super::{
    Chats, SignedOut,
    jump::{Jump, JumpEvent, Place},
    keys::{self, Move},
};
use crate::{
    chat::chat_task_id,
    link::host_agent,
    sheets::{
        self, Host, Member, Page, SheetEvent, apps::AppsSheet, memory::Memory, routines::Routines,
        settings::Settings,
    },
    theme::Palette,
};

/// How long a sheet takes to rise into place.
const RISE: Duration = Duration::from_millis(280);

/// A sheet that is up.
pub(super) struct Shown {
    page: Page,
    view: AnyView,
    /// Counts the sheets opened, so each rises afresh.
    opened: usize,
    _events: Subscription,
}

/// The ⌘K box, while it is up.
pub(super) struct Jumping {
    jump: Entity<Jump>,
    _events: Subscription,
}

impl Chats {
    /// The Host's agents as the sheets show them.
    fn members(&self) -> Vec<Member> {
        self.agents
            .iter()
            .filter_map(|agent| {
                let host_id = host_agent(agent.target.as_str())?;
                let (look, primary) = self.face(agent);
                Some(Member {
                    name: agent.name.clone().into(),
                    host_id,
                    task_id: chat_task_id(agent.node_id, agent.target.as_str()),
                    look,
                    primary,
                })
            })
            .collect()
    }

    pub(super) fn open_sheet(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.as_ref().is_some_and(|shown| shown.page == page) {
            return;
        }
        self.jumping = None;
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
                let sheet = cx.new(|_| Settings::new(self.computer.clone()));
                let events = cx.subscribe_in(&sheet, window, Self::on_sheet);
                (AnyView::from(sheet), events)
            }
        };
        self.sheets_opened += 1;
        self.sheet = Some(Shown {
            page,
            view,
            opened: self.sheets_opened,
            _events: events,
        });
        // Keys reach the chats while the sheet is up, so Esc closes it.
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
                if let Some(index) = self.agents.iter().position(|agent| {
                    chat_task_id(agent.node_id, agent.target.as_str()) == *task_id
                }) {
                    self.select(index, window, cx);
                }
            }
            SheetEvent::SignOut => cx.emit(SignedOut),
        }
    }

    /// Closes whatever is up and gives the keys back to the composer.
    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.jumping = None;
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        cx.notify();
    }

    pub(super) fn open_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        let mut places: Vec<Place> = self
            .agents
            .iter()
            .enumerate()
            .map(|(index, agent)| {
                let (look, primary) = self.face(agent);
                Place::Agent {
                    index,
                    name: agent.name.clone().into(),
                    look,
                    primary,
                }
            })
            .collect();
        places.extend(Page::ALL.map(Place::Sheet));
        let jump = cx.new(|cx| Jump::new(places, window, cx));
        let events = cx.subscribe_in(&jump, window, |this, _, event: &JumpEvent, window, cx| {
            let JumpEvent::Go(place) = event;
            match place {
                Place::Agent { index, .. } => {
                    this.close(window, cx);
                    this.select(*index, window, cx);
                }
                Place::Sheet(page) => this.open_sheet(*page, window, cx),
            }
        });
        self.jumping = Some(Jumping {
            jump,
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
        let Some(moved) = keys::shortcut(&event.keystroke) else {
            return false;
        };
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
                self.select(index, window, cx);
            }
        }
        true
    }

    /// The sheet or the ⌘K box over the chats, on a scrim that closes it.
    pub(super) fn overlay(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let scrim = || {
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .bg(palette.scrim)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )
        };
        let card = |content: AnyView| {
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content)
        };
        if let Some(jumping) = &self.jumping {
            return Some(
                scrim()
                    .items_start()
                    .pt(px(110.))
                    .child(card(jumping.jump.clone().into()))
                    .into_any_element(),
            );
        }
        let shown = self.sheet.as_ref()?;
        Some(
            scrim()
                .items_center()
                .child(
                    card(shown.view.clone())
                        .relative()
                        .w(px(sheets::WIDTH))
                        .h(px(sheets::HEIGHT))
                        .max_h_full()
                        .rounded(px(16.))
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.outline)
                        .shadow_2xl()
                        .overflow_hidden()
                        .with_animation(
                            ("sheet", shown.opened),
                            Animation::new(RISE).with_easing(ease_out_quint()),
                            |card, t| card.top(px(18. * (1. - t))).opacity(t),
                        ),
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
        let up = self.sheet.as_ref().map(|shown| shown.page);
        div()
            .flex()
            .gap(px(2.))
            .p(px(6.))
            .border_t_1()
            .border_color(palette.outline)
            .children(Page::ALL.map(|page| {
                div()
                    .id(SharedString::from(page.title()))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(palette.secondary)
                    .hover(|button| button.bg(palette.selected).text_color(palette.text))
                    .when(up == Some(page), |button| {
                        button.bg(palette.selected).text_color(palette.text)
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_sheet(page, window, cx)),
                    )
                    .child(div().text_size(px(15.)).child(page.icon()))
                    .child(
                        div()
                            .text_size(px(10.))
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
            .mx(px(2.))
            .mb(px(10.))
            .h(px(30.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.selected)
            .text_color(palette.secondary)
            .cursor_pointer()
            .hover(|button| button.text_color(palette.text))
            .on_click(cx.listener(|this, _, window, cx| this.open_jump(window, cx)))
            .child(gpui_kit::assets::IconName::Search)
            .child(div().flex_1().child("Jump to…"))
            .child(div().text_size(px(11.)).child("⌘K"))
    }
}
