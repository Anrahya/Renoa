//! The open conversation: its agent's name over a fade, the lines with the
//! time between those far apart, the agent at work, and the composer.

use std::time::Duration;

use chrono::Local;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Context, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestSupportExt as _, Window,
    canvas,
    component::{input::Input, message_scroller::MessageScroller, text::TextView},
    div, ease_out_quint, linear_color_stop, linear_gradient, point,
    prelude::FluentBuilder as _,
    relative,
};

use super::{Chats, Row};
use crate::{
    chat::{Line, Speaker, Transcript, chat_task_id},
    face::{Face, Look, Mood},
    stamp,
    theme::Palette,
};

use crate::theme::u;

/// The widest the conversation runs.
const COLUMN: f32 = 720.;

/// The height of the name over the conversation, which the lines scroll under.
const HEADER: f32 = 64.;

/// An owner's new line rises into place; an agent's settles.
const RISE: Duration = Duration::from_millis(420);
const SETTLE: Duration = Duration::from_millis(500);

/// One beat of the working dots.
const DOTS: Duration = Duration::from_millis(1100);

/// The dots need no more frames than this a second.
const DOT_FPS: f32 = 30.;

/// Who is talking in the open chat, as each row draws them.
#[derive(Clone)]
struct Agent {
    name: SharedString,
    look: Option<Look>,
    primary: bool,
    /// Whether its eyes are shut just now.
    blinking: bool,
}

impl Agent {
    fn face(&self, palette: Palette, size: f32) -> Face {
        Face::new(self.look, self.primary, palette, u(size)).blinking(self.blinking)
    }
}

impl Chats {
    pub(super) fn conversation(
        &self,
        palette: Palette,
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
        let notice = task_id
            .filter(|task_id| self.unsent.contains(task_id))
            .map(|_| "Your message didn't send. Try again.");
        // Rows read the chat as they are drawn; only the visible ones are.
        let chats = cx.entity().downgrade();
        let speaking = agent.clone();
        let thread = MessageScroller::new("thread", self.scroller.clone(), move |index, _, cx| {
            chats.upgrade().map_or_else(
                || div().into_any_element(),
                |chats| row(index, chats.read(cx).row(index), &speaking, palette),
            )
        })
        .with_list_style(padded(StyleRefinement::default()))
        // Each row owns the space above it.
        .with_row_style(StyleRefinement::default().pb_0().px_0())
        .flex_1();
        let sendable = self.linked && !self.composer.read(cx).value().trim().is_empty();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(thread)
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
                        .text_color(palette.danger)
                        .text_size(u(12.))
                        .child(notice),
                ))
            })
            .child(centred(
                div()
                    .w_full()
                    .m(u(12.))
                    .flex()
                    .items_end()
                    .gap(u(8.))
                    .pl(u(16.))
                    .pr(u(6.))
                    .py(u(6.))
                    .rounded(u(22.))
                    .bg(palette.composer)
                    .border_1()
                    .border_color(palette.outline)
                    .child(
                        div().flex_1().py(u(4.)).child(
                            Input::new(&self.composer)
                                .id("composer")
                                .aria_label("Message")
                                .appearance(false),
                        ),
                    )
                    .child(
                        send_button(sendable, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                    ),
            ))
    }
}

/// Room at the top for the name the lines scroll under.
fn padded(mut style: StyleRefinement) -> StyleRefinement {
    style.padding.top = Some(u(HEADER).into());
    style.padding.bottom = Some(u(8.).into());
    style
}

/// `content` in the middle of the conversation, no wider than [`COLUMN`].
fn centred(content: impl IntoElement) -> gpui_kit::Div {
    div().w_full().flex().justify_center().px(u(20.)).child(
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
        .flex()
        .items_center()
        .justify_center()
        .gap(u(8.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(palette.background, 0.55),
            linear_color_stop(clear, 1.),
        ))
        .child(
            div()
                .id("about")
                .flex()
                .items_center()
                .gap(u(8.))
                .max_w(u(COLUMN * 0.8))
                .pl(u(5.))
                .pr(u(14.))
                .py(u(4.))
                .rounded_full()
                .bg(palette.frosted)
                .border_1()
                .border_color(palette.outline)
                .when_some(about, |pill, about| {
                    pill.cursor_pointer()
                        .hover(|pill| pill.bg(palette.selected))
                        .on_click(about)
                })
                .test_support()
                .child(agent.face(palette, 26.).mood(mood).alive("header-face"))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(agent.name.clone()),
                )
                .when_some(purpose, |pill, purpose| {
                    pill.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(palette.secondary)
                            .child(purpose),
                    )
                }),
        )
        .when(!linked, |header| {
            header.child(
                div()
                    .text_color(palette.attention)
                    .text_size(u(12.))
                    .child("Reconnecting…"),
            )
        })
}

/// One row of the conversation: a line, with the time before it when one
/// goes there; past the last line, the agent at work.
fn row(index: usize, row: Row, agent: &Agent, palette: Palette) -> AnyElement {
    let Some(line) = row.line else {
        return centred(working(agent, palette)).into_any_element();
    };
    let at = line.at_ms;
    let time = stamp::separates(
        row.before.as_ref().and_then(|before| before.at_ms),
        at,
        &Local,
    )
    .then(|| at.map(|at_ms| stamp::long(at_ms, &Local::now())))
    .flatten();
    // A new speaker starts a little further down than the same one going on.
    let turned = row
        .before
        .as_ref()
        .is_none_or(|before| before.speaker != line.speaker);
    let said = match line.speaker {
        Speaker::Owner => owner(index, &line, row.live, palette),
        Speaker::Agent => said_by_agent(
            index,
            &line,
            row.after != Some(Speaker::Agent),
            row.live,
            agent,
            palette,
        ),
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .pt(u(if turned { 10. } else { 2. }))
        .when_some(time, |column, time| {
            column.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .py(u(8.))
                    .text_size(u(11.))
                    .text_color(palette.secondary)
                    .child(time),
            )
        })
        .child(centred(said))
        .into_any_element()
}

fn owner(index: usize, line: &Line, live: bool, palette: Palette) -> AnyElement {
    let bubble = div()
        .max_w(relative(0.72))
        .px(u(14.))
        .py(u(9.))
        .rounded(u(18.))
        .bg(palette.chat_user)
        .child(SharedString::from(line.text.clone()));
    let placed = div().w_full().flex().justify_end().child(bubble);
    if !live {
        return placed.into_any_element();
    }
    placed
        .relative()
        .with_animation(
            ("rise", index),
            Animation::new(RISE).with_easing(ease_out_quint()),
            |placed, t| placed.top(u(22. * (1. - t))).opacity(t),
        )
        .into_any_element()
}

/// An agent's line, as plain text without a bubble, with its face beside the
/// `last` line of a run.
fn said_by_agent(
    index: usize,
    line: &Line,
    last: bool,
    live: bool,
    agent: &Agent,
    palette: Palette,
) -> AnyElement {
    let text = div().max_w(relative(0.85)).min_w_0().py(u(2.)).map(|text| {
        if line.failed {
            text.text_color(palette.danger)
                .child(SharedString::from(line.text.clone()))
        } else {
            text.child(TextView::markdown(("said", index), line.text.clone()))
        }
    });
    let face = div().w(u(24.)).flex_none().when(last, |slot| {
        slot.child(agent.face(palette, 24.).alive(format!("said-{index}")))
    });
    let placed = div()
        .w_full()
        .flex()
        .items_end()
        .gap(u(10.))
        .child(face)
        .child(text);
    if !live {
        return placed.into_any_element();
    }
    placed
        .relative()
        .with_animation(
            ("settle", index),
            Animation::new(SETTLE).with_easing(ease_out_quint()),
            |placed, t| placed.top(u(8. * (1. - t))).opacity(t),
        )
        .into_any_element()
}

/// The agent at work: its face bobbing, and dots that come and go in turn.
fn working(agent: &Agent, palette: Palette) -> impl IntoElement {
    let dots = (0..3_u8).map(move |dot| {
        let offset = f32::from(dot) * 0.18;
        div()
            .size(u(5.))
            .rounded_full()
            .bg(palette.secondary)
            .with_animation(
                ("dot", usize::from(dot)),
                Animation::new(DOTS).repeat().with_max_fps(DOT_FPS),
                move |dot, t| {
                    // Bright at its moment in the beat, faint the rest.
                    let at = (t - offset).rem_euclid(1.);
                    dot.opacity(0.3 + 0.7 * (1. - (at * 2. - 0.5).abs().min(1.)))
                },
            )
    });
    div()
        .w_full()
        .pt(u(10.))
        .flex()
        .items_end()
        .gap(u(8.))
        .child(
            agent
                .face(palette, 24.)
                .mood(Mood::Working)
                .alive("working-face"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .py(u(2.))
                .text_color(palette.secondary)
                .child(format!("{} is working on it", agent.name))
                .child(div().flex().gap(u(3.)).children(dots)),
        )
}

/// The round send button: ink when there is something to send, faint when not.
fn send_button(sendable: bool, palette: Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (fill, arrow) = if sendable {
        (palette.text, palette.background)
    } else {
        (palette.outline, palette.background)
    };
    div()
        .id("send")
        .size(u(30.))
        .flex_none()
        .rounded_full()
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .when(sendable, |button| button.cursor_pointer())
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
                    let mut up = PathBuilder::stroke(u(2.).to_pixels(rem));
                    up.move_to(at(7., 12.5));
                    up.line_to(at(7., 2.));
                    up.move_to(at(2.5, 6.5));
                    up.line_to(at(7., 2.));
                    up.line_to(at(11.5, 6.5));
                    if let Ok(path) = up.build() {
                        window.paint_path(path, arrow);
                    }
                },
            )
            .size(u(14.)),
        )
}
