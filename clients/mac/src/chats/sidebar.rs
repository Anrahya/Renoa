//! The side of the chats: the owner's agents, each with its face, the last
//! thing said and when, and a highlight that springs to the open one.

use std::time::Duration;

use chrono::Local;
use gpui_kit::{
    Animation, AnimationExt as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, SpringAnimation, SpringConfig,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, assets::IconName, div,
    ease_out_quint, prelude::FluentBuilder as _,
};

use super::Chats;
use crate::{
    chat::{Transcript, chat_task_id},
    face::{self, Face, Mood},
    stamp,
    theme::Palette,
};

use crate::theme::u;

/// The width of the side.
const WIDTH: f32 = 264.;

/// The height of an agent's row.
const ROW: f32 = 56.;

/// Space for the thin divider below the pinned main agent row.
const PIN_GAP: f32 = 13.;

/// How fast the sidebar slides open or closed.
const SLIDE: Duration = Duration::from_millis(220);

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
            .position(|agent| Some(chat_task_id(agent.node_id, agent.target.as_str())) == chosen)
            .filter(|_| !self.on_home);
        let now = Local::now();
        let has_pin_gap = self.agents.len() > 1;
        let rows = self.agents.iter().enumerate().flat_map(|(index, agent)| {
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
            let is_open = open == Some(index);
            let row = div()
                .id(("agent", index))
                .relative()
                .h(u(ROW))
                .flex()
                .items_center()
                .gap(u(10.))
                .px(u(10.))
                .rounded(u(10.))
                .cursor_pointer()
                .when(primary, |row| {
                    row.border_1().border_color(palette.attention_border)
                })
                .when(primary && !is_open, |row| {
                    row.bg(palette.attention_wash)
                        .hover(|row| row.bg(palette.hover).border_color(palette.attention))
                })
                .when(!primary && !is_open, |row| {
                    row.hover(|row| row.bg(palette.hover))
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_agent(index, window, cx)),
                )
                .child(
                    Face::new(look, primary, palette, u(30.))
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
                        .gap(u(1.))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(u(6.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(u(5.))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(SharedString::from(agent.name.clone())),
                                        )
                                        .when(primary, |title| {
                                            title.child(face::star(10.5, palette.attention))
                                        }),
                                )
                                .when_some(at, |line, at| {
                                    line.child(
                                        div()
                                            .flex_none()
                                            .text_size(u(11.))
                                            .text_color(palette.secondary)
                                            .child(at),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(u(12.))
                                .text_color(palette.secondary)
                                .truncate()
                                .child(if working {
                                    SharedString::from("Working on it…")
                                } else {
                                    self.preview(task_id)
                                }),
                        ),
                )
                .test_support()
                .into_any_element();
            let divider = (index == 0 && has_pin_gap).then(|| {
                div()
                    .my(u(6.))
                    .mx(u(8.))
                    .h(u(1.))
                    .bg(palette.divider)
                    .into_any_element()
            });
            std::iter::once(row).chain(divider)
        });
        let empty = self.agents.is_empty();
        let list =
            div()
                .id("agents")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(u(8.))
                .child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .when_some(open, |list, index| {
                            list.child(highlight(index, has_pin_gap, palette))
                        })
                        .children(rows),
                )
                .when(empty, |list| {
                    list.child(div().px(u(10.)).text_color(palette.secondary).child(
                        if self.linked {
                            "No agents yet. Make one in Renoa on your phone."
                        } else {
                            "Connecting to Renoa…"
                        },
                    ))
                });
        let panel = div()
            .w(u(WIDTH))
            .h_full()
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            // Room for the window's traffic lights.
            .pt(u(44.))
            .bg(palette.sidebar)
            .border_r_1()
            .border_color(palette.outline)
            .when(self.sidebar_open, |panel| {
                panel.child(
                    div()
                        .id("toggle-sidebar")
                        .absolute()
                        .top(u(9.))
                        .right(u(10.))
                        .size(u(28.))
                        .rounded(u(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(palette.secondary)
                        .cursor_pointer()
                        .hover(|button| button.bg(palette.hover).text_color(palette.text))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)),
                        )
                        .child(IconName::PanelLeft)
                        .test_support(),
                )
            })
            .child(div().px(u(8.)).child(self.jump_button(palette, cx)))
            .child(div().pt(u(8.)).child(self.home_row(palette, cx)))
            .child(
                div()
                    .px(u(18.))
                    .pt(u(10.))
                    .pb(u(6.))
                    .text_size(u(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.secondary)
                    .child("Agents"),
            )
            .child(list)
            .child(self.sheet_buttons(palette, cx));
        let shell = div().h_full().flex_none().overflow_hidden().child(panel);
        let open = self.sidebar_open;
        if self.sidebar_toggled == 0 {
            return shell
                .w(if open { u(WIDTH) } else { u(0.) })
                .into_any_element();
        }
        shell
            .with_animation(
                ("sidebar", self.sidebar_toggled),
                Animation::new(SLIDE).with_easing(ease_out_quint()),
                move |shell, t| {
                    let shown = if open { t } else { 1. - t };
                    shell.w(u(WIDTH * shown)).opacity(shown)
                },
            )
            .into_any_element()
    }

    /// Floating expand button beside the traffic lights while the sidebar is collapsed.
    pub(super) fn reopen_sidebar(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button = div()
            .id("toggle-sidebar")
            .absolute()
            .top(u(9.))
            .left(u(82.))
            .size(u(28.))
            .rounded(u(8.))
            .bg(palette.frosted)
            .border_1()
            .border_color(palette.outline)
            .shadow_2xs()
            .flex()
            .items_center()
            .justify_center()
            .text_color(palette.secondary)
            .cursor_pointer()
            .hover(|button| {
                button
                    .bg(palette.selected)
                    .border_color(palette.outline_strong)
                    .text_color(palette.text)
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)))
            .child(IconName::PanelLeftOpen)
            .test_support();
        button.with_animation(
            ("reopen-sidebar", self.sidebar_toggled),
            Animation::new(SLIDE).with_easing(ease_out_quint()),
            |button, t| button.opacity(t),
        )
    }
}

/// The open chat's highlight, behind the row at `index`.
fn highlight(index: usize, has_pin_gap: bool, palette: Palette) -> impl IntoElement {
    let gap = if index > 0 && has_pin_gap {
        PIN_GAP
    } else {
        0.
    };
    let top = u(f32::from(u16::try_from(index).unwrap_or(u16::MAX)) * ROW + gap);
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(u(ROW))
        .rounded(u(10.))
        .bg(palette.selected)
        .with_spring(
            "highlight",
            SpringAnimation::new(SPRING).to(top),
            |highlight, top| highlight.top(top),
        )
}
