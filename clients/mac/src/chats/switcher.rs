//! The Ctrl+Tab switcher: while Ctrl is held, a card of the chats the owner
//! opened, most recent first, with the one the walk reached marked. Letting
//! go of Ctrl opens it (`keys.rs`); Esc closes the card without moving.

use gpui_kit::{
    FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::FluentBuilder as _,
};

use super::Chats;
use crate::{chat::chat_task_id, face::Face, theme::Palette};

use crate::theme::u;

impl Chats {
    /// The switcher card, centred over the chats, while a walk goes on.
    pub(super) fn switcher(&self, palette: Palette) -> Option<impl IntoElement + use<>> {
        let (opened, at) = self.recent.walking()?;
        let rows = opened.iter().enumerate().filter_map(|(place, task)| {
            let agent = self
                .agents
                .iter()
                .find(|agent| chat_task_id(agent.node_id, agent.target.as_str()) == *task)?;
            let (look, primary) = self.face(agent);
            Some(
                div()
                    .h(u(40.))
                    .px(u(10.))
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .rounded(u(10.))
                    .when(place == at, |row| row.bg(palette.selected))
                    .child(Face::new(look, primary, palette, u(24.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(agent.name.clone()),
                    ),
            )
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(u(320.))
                        .flex()
                        .flex_col()
                        .rounded(u(16.))
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.outline)
                        .shadow_2xl()
                        .overflow_hidden()
                        .child(div().p(u(6.)).flex().flex_col().gap(u(2.)).children(rows))
                        .child(
                            div()
                                .px(u(16.))
                                .py(u(8.))
                                .border_t_1()
                                .border_color(palette.divider)
                                .text_size(u(11.))
                                .text_color(palette.secondary)
                                .child("Tab to move · let go of Ctrl to open · Esc to stay"),
                        ),
                ),
        )
    }
}
