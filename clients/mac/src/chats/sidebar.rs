//! The side of the chats: the owner's agents, each with its face, the last
//! thing said and when, and a highlight that springs to the open one.

use chrono::Local;
use gpui_kit::{
    AnimationExt as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, SpringAnimation, SpringConfig,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

use super::Chats;
use crate::{
    chat::{Transcript, chat_task_id},
    face::{Face, Mood},
    stamp,
    theme::Palette,
};

/// The width of the side.
const WIDTH: f32 = 264.;

/// The height of an agent's row.
const ROW: f32 = 56.;

/// How the highlight moves to the open chat: a little past it and back, in
/// about half a second.
const SPRING: SpringConfig = SpringConfig::new(320., 24., 1.);

impl Chats {
    pub(super) fn sidebar(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let chosen = self.chosen_task();
        let open = self
            .agents
            .iter()
            .position(|agent| Some(chat_task_id(agent.node_id, agent.target.as_str())) == chosen);
        let now = Local::now();
        let rows = self.agents.iter().enumerate().map(|(index, agent)| {
            let task_id = chat_task_id(agent.node_id, agent.target.as_str());
            let transcript = self.transcripts.get(&task_id);
            let working = transcript.is_some_and(Transcript::working);
            let at = transcript
                .and_then(|transcript| transcript.lines().iter().rev().find_map(|line| line.at_ms))
                .map(|at_ms| SharedString::from(stamp::short(at_ms, &now)));
            let (look, primary) = self.face(agent);
            let mood = if working {
                Mood::Working
            } else {
                Mood::Resting
            };
            div()
                .id(("agent", index))
                .relative()
                .h(px(ROW))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .rounded(px(10.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.select(index, window, cx)))
                .child(
                    Face::new(look, primary, palette, px(30.))
                        .blinking(self.blinking(task_id))
                        .mood(mood)
                        .alive(format!("side-{task_id}")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(SharedString::from(agent.name.clone())),
                                )
                                .when_some(at, |line, at| {
                                    line.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(palette.secondary)
                                            .child(at),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.secondary)
                                .truncate()
                                .child(if working {
                                    SharedString::from("Working on it…")
                                } else {
                                    self.preview(task_id)
                                }),
                        ),
                )
        });
        let empty = self.agents.is_empty();
        let list =
            div()
                .id("agents")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(8.))
                .child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .when_some(open, |list, index| list.child(highlight(index, palette)))
                        .children(rows),
                )
                .when(empty, |list| {
                    list.child(div().px(px(10.)).text_color(palette.secondary).child(
                        if self.linked {
                            "No agents yet. Make one in Renoa on your phone."
                        } else {
                            "Connecting to Renoa…"
                        },
                    ))
                });
        div()
            .w(px(WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            // Room for the window's traffic lights.
            .pt(px(44.))
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.outline)
            .child(div().px(px(8.)).child(self.jump_button(palette, cx)))
            .child(
                div()
                    .px(px(18.))
                    .pb(px(6.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.secondary)
                    .child("Agents"),
            )
            .child(list)
            .child(self.sheet_buttons(palette, cx))
    }
}

/// The open chat's highlight, behind the row at `index`.
fn highlight(index: usize, palette: Palette) -> impl IntoElement {
    let top = px(f32::from(u16::try_from(index).unwrap_or(u16::MAX)) * ROW);
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(ROW))
        .rounded(px(10.))
        .bg(palette.selected)
        .with_spring(
            "highlight",
            SpringAnimation::new(SPRING).to(top),
            |highlight, top| highlight.top(top),
        )
}
