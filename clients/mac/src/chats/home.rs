//! Home's place in the chats: its row at the top of the side, the way to it
//! and back to a chat, and what it asks of the chats.

use gpui_kit::{
    Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window, assets::IconName,
    div, prelude::FluentBuilder as _, px,
};

use super::Chats;
use crate::{
    home::{Home, HomeEvent},
    theme::Palette,
};

impl Chats {
    pub(super) fn home_pane(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .track_focus(&self.home_focus)
            .child(self.home.clone())
    }

    /// Shows Home, read afresh.
    pub(super) fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.jumping = None;
        self.on_home = true;
        window.focus(&self.home_focus, cx);
        self.home.update(cx, |home, cx| home.read(cx));
        cx.notify();
    }

    /// Opens the chat with the agent at `index`, leaving Home, ready to type.
    pub(super) fn open_agent(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.agents.len() {
            return;
        }
        self.on_home = false;
        self.select(index, window, cx);
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        cx.notify();
    }

    /// Gives the keys back to whatever shows: Home, or the chat's composer.
    pub(super) fn refocus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_home {
            window.focus(&self.home_focus, cx);
        } else {
            self.composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
    }

    pub(super) fn on_home(
        &mut self,
        _: &Entity<Home>,
        event: &HomeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match *event {
            HomeEvent::Open(task_id) => self.open_chat(task_id, window, cx),
            HomeEvent::Sheet(page) => self.open_sheet(page, window, cx),
        }
    }

    /// Home's row at the top of the side, with how many things wait on the
    /// owner.
    pub(super) fn home_row(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let waiting = self.home.read(cx).needing_you();
        div()
            .id("home-row")
            .mx(px(8.))
            .h(px(48.))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(self.on_home, |row| row.bg(palette.selected))
            .when(!self.on_home, |row| {
                row.hover(|row| row.bg(palette.selected))
            })
            .on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx)))
            .child(
                div()
                    .size(px(30.))
                    .flex_none()
                    .rounded(px(9.))
                    .bg(palette.chat_agent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(IconName::House),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Home"))
                    .when_some(waiting, |words, waiting| {
                        words.child(
                            div()
                                .text_size(px(12.))
                                .text_color(if waiting > 0 {
                                    palette.attention
                                } else {
                                    palette.secondary
                                })
                                .child(match waiting {
                                    0 => "All caught up".to_owned(),
                                    1 => "1 thing needs you".to_owned(),
                                    many => format!("{many} things need you"),
                                }),
                        )
                    }),
            )
            .test_support()
    }
}
