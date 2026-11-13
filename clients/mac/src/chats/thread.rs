//! The open conversation: its agent's name over a fade, the lines with the
//! time between those far apart, the agent at work, and the composer.

mod prose;

use chrono::Local;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Focusable as _, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, PathBuilder, PathStyle, SharedString,
    StatefulInteractiveElement as _, StrokeOptions, StyleRefinement, Styled as _,
    TestSupportExt as _, Window, canvas,
    component::{input::Input, message_scroller::MessageScroller},
    div, linear_color_stop, linear_gradient, point,
    prelude::FluentBuilder as _,
};
use lyon_tessellation::{LineCap, LineJoin};

use super::{Chats, Row};
use crate::{
    chat::{Speaker, Transcript, chat_task_id},
    face::{Face, Look, Mood},
    stamp,
    theme::{Palette, u},
};

/// The widest the conversation runs.
const COLUMN: f32 = 680.;

/// The height of the name over the conversation, which the lines scroll under.
const HEADER: f32 = 76.;

/// Who is talking in the open chat, as each row draws them.
#[derive(Clone)]
pub(super) struct Agent {
    pub(super) name: SharedString,
    look: Option<Look>,
    primary: bool,
    /// Whether its eyes are shut just now.
    blinking: bool,
}

impl Agent {
    pub(super) fn face(&self, palette: Palette, size: f32) -> Face {
        Face::new(self.look, self.primary, palette, u(size)).blinking(self.blinking)
    }
}

impl Chats {
    pub(super) fn conversation(
        &self,
        palette: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let chosen = self.chosen.as_ref();
        let task_id = self.chosen_task();
        let listed = task_id.and_then(|task_id| {
            self.agents
                .iter()
                .find(|agent| chat_task_id(agent.node_id, agent.target.as_str()) == task_id)
        });
        let (look, primary) = listed.map_or((None, false), |agent| self.face(agent));
        let agent = Agent {
            name: chosen.map_or_else(SharedString::default, |chosen| chosen.name.clone()),
            look,
            primary,
            blinking: task_id.is_some_and(|task_id| self.blinking(task_id)),
        };
        let purpose = task_id.and_then(|task_id| self.purposes.get(&task_id).cloned());
        let working = self.transcript().is_some_and(Transcript::working);
        let empty = chosen.is_some() && self.rows() == 0 && !working;
        let notice = task_id
            .filter(|task_id| self.unsent.contains(task_id))
            .map(|_| "Your message didn't send. Try again.");
        // Rows read the chat as they are drawn; only the visible ones are.
        let chats = cx.entity().downgrade();
        let speaking = agent.clone();
        let card_bg = palette.card;
        let card_border = palette.outline;
        let card_text = palette.text;
        let thread = MessageScroller::new("thread", self.scroller.clone(), move |index, _, cx| {
            chats.upgrade().map_or_else(
                || div().into_any_element(),
                |chats| row(index, chats.read(cx).row(index), &speaking, palette),
            )
        })
        .with_list_style(padded(StyleRefinement::default()))
        // Each row owns the space above it.
        .with_row_style(StyleRefinement::default().pb_0().px_0())
        .with_bottom_fade(palette.background)
        .with_jump_button_renderer(move |button| {
            button
                .bg(card_bg)
                .border_color(card_border)
                .text_color(card_text)
                .shadow_md()
        })
        .flex_1();
        let sendable = self.linked && !self.composer.read(cx).value().trim().is_empty();
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(thread)
            .when(empty, |column| {
                column.child(empty_chat(&agent, purpose.clone(), palette))
            })
            .when(chosen.is_some(), |column| {
                let about = self.profile_task().map(|task_id| {
                    cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| {
                        this.open_profile(task_id, window, cx);
                    })
                });
                column.child(header(
                    &agent,
                    purpose,
                    working,
                    self.linked,
                    about,
                    palette,
                ))
            })
            .when_some(notice, |column, notice| {
                column.child(centred(
                    div()
                        .px(u(20.))
                        .pb(u(4.))
                        .text_color(palette.danger)
                        .text_size(u(12.))
                        .child(notice),
                ))
            })
            .child(centred(
                div()
                    .w_full()
                    .mt(u(6.))
                    .mb(u(16.))
                    .flex()
                    .items_end()
                    .gap(u(10.))
                    .pl(u(18.))
                    .pr(u(7.))
                    .py(u(7.))
                    .rounded(u(23.))
                    .bg(palette.composer)
                    .border_1()
                    .border_color(if focused {
                        palette.outline_strong
                    } else {
                        palette.outline
                    })
                    .shadow_md()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h(u(32.))
                            .flex()
                            .items_center()
                            .child(
                                div().w_full().child(
                                    Input::new(&self.composer)
                                        .id("composer")
                                        .aria_label("Message")
                                        .appearance(false),
                                ),
                            ),
                    )
                    .child(
                        send_button(sendable, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            ))
    }
}

/// Shown when a chat has no messages yet so a fresh agent feels welcoming.
fn empty_chat(agent: &Agent, purpose: Option<SharedString>, palette: Palette) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .pb(u(64.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(u(10.))
        .px(u(24.))
        .child(agent.face(palette, 48.).alive("empty-face"))
        .child(
            div()
                .text_size(u(19.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child(agent.name.clone()),
        )
        .when_some(purpose, |box_, purpose| {
            box_.child(
                div()
                    .max_w(u(380.))
                    .text_center()
                    .text_size(u(13.5))
                    .line_height(u(20.))
                    .text_color(palette.secondary)
                    .child(purpose),
            )
        })
}

/// Room at the top for the name the lines scroll under, and breathing space above
/// the composer at the bottom.
fn padded(mut style: StyleRefinement) -> StyleRefinement {
    style.padding.top = Some(u(HEADER).into());
    style.padding.bottom = Some(u(20.).into());
    style
}

/// `content` in the middle of the conversation, no wider than [`COLUMN`].
fn centred(content: impl IntoElement) -> gpui_kit::Div {
    div().w_full().flex().justify_center().px(u(24.)).child(
        div()
            .w_full()
            .max_w(u(COLUMN))
            .flex()
            .justify_center()
            .child(content),
    )
}

/// The agent's face, name and purpose in a pill over a fade the lines go
/// under; clicking the pill opens `about`, the agent's page.
fn header(
    agent: &Agent,
    purpose: Option<SharedString>,
    working: bool,
    linked: bool,
    about: Option<impl Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    palette: Palette,
) -> impl IntoElement {
    let mood = if working {
        Mood::Working
    } else {
        Mood::Resting
    };
    let mut clear = palette.background;
    clear.a = 0.;
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(u(HEADER))
        .pt(u(14.))
        .flex()
        .items_start()
        .justify_center()
        .gap(u(8.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(palette.background, 0.5),
            linear_color_stop(clear, 1.),
        ))
        .child(
            div()
                .id("about")
                .flex()
                .items_center()
                .gap(u(8.))
                .max_w(u(COLUMN * 0.82))
                .pl(u(6.))
                .pr(u(14.))
                .py(u(5.))
                .rounded_full()
                .bg(palette.frosted)
                .border_1()
                .border_color(palette.outline)
                .shadow_xs()
                .when_some(about, |pill, about| {
                    pill.cursor_pointer()
                        .hover(|pill| {
                            pill.bg(palette.selected)
                                .border_color(palette.outline_strong)
                        })
                        .on_click(about)
                })
                .test_support()
                .child(agent.face(palette, 24.).mood(mood).alive("header-face"))
                .child(
                    div()
                        .flex_none()
                        .text_size(u(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(agent.name.clone()),
                )
                .when_some(purpose, |pill, purpose| {
                    pill.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(u(12.5))
                            .text_color(palette.secondary)
                            .child(purpose),
                    )
                }),
        )
        .when(!linked, |header| {
            header.child(
                div()
                    .px(u(10.))
                    .py(u(5.))
                    .rounded_full()
                    .bg(palette.attention_wash)
                    .border_1()
                    .border_color(palette.attention_border)
                    .text_color(palette.attention)
                    .text_size(u(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Reconnecting…"),
            )
        })
}

/// One row of the conversation: a line, with the time before it when one
/// goes there; past the last line, the agent at work.
fn row(index: usize, row: Row, agent: &Agent, palette: Palette) -> AnyElement {
    let Some(line) = row.line else {
        return centred(prose::working(agent, palette)).into_any_element();
    };
    let at = line.at_ms;
    let time = stamp::separates(
        row.before.as_ref().and_then(|before| before.at_ms),
        at,
        &Local,
    )
    .then(|| at.map(|at_ms| stamp::long(at_ms, &Local::now())))
    .flatten();
    let top_pad = prose::row_top_padding(row.before.as_ref().map(|b| b.speaker), line.speaker);
    let said = match line.speaker {
        Speaker::Owner => prose::owner(index, &line, row.live, palette),
        Speaker::Agent => prose::agent_line(index, &line, row.live, palette),
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .pt(u(top_pad))
        .when_some(time, |column, time| {
            column.child(centred(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .py(u(10.))
                    .child(div().flex_1().h(u(1.)).bg(palette.divider))
                    .child(
                        div()
                            .text_size(u(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.secondary)
                            .child(time),
                    )
                    .child(div().flex_1().h(u(1.)).bg(palette.divider)),
            ))
        })
        .child(centred(said))
        .into_any_element()
}

/// The round send button: ink when there is something to send, muted when not.
fn send_button(sendable: bool, palette: Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (fill, arrow) = if sendable {
        (palette.text, palette.background)
    } else {
        (palette.chat_agent, palette.secondary.opacity(0.6))
    };
    div()
        .id("send")
        .size(u(32.))
        .flex_none()
        .rounded_full()
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .when(sendable, |button| {
            button.cursor_pointer().hover(|button| button.opacity(0.84))
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let rem = window.rem_size();
                    let at = |x: f32, y: f32| {
                        point(
                            bounds.origin.x + u(x).to_pixels(rem),
                            bounds.origin.y + u(y).to_pixels(rem),
                        )
                    };
                    let style = StrokeOptions::default()
                        .with_line_width(f32::from(u(1.85).to_pixels(rem)))
                        .with_line_cap(LineCap::Round)
                        .with_line_join(LineJoin::Round);
                    let mut up = PathBuilder::fill().with_style(PathStyle::Stroke(style));
                    up.move_to(at(7., 11.25));
                    up.line_to(at(7., 2.75));
                    up.move_to(at(3., 6.75));
                    up.line_to(at(7., 2.75));
                    up.line_to(at(11., 6.75));
                    if let Ok(path) = up.build() {
                        window.paint_path(path, arrow);
                    }
                },
            )
            .size(u(14.)),
        )
}
