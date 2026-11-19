//! The chat composer: message input, send button, and stop control for an active turn.

use gpui_kit::{
    ClickEvent, Context, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _,
    PathBuilder, PathStyle, StatefulInteractiveElement as _, StrokeOptions, Styled as _,
    TestSupportExt as _, Window, canvas, component::input::Input, div, point,
    prelude::FluentBuilder as _,
};
use lyon_tessellation::{LineCap, LineJoin};
use renoa_protocol::CommandId;
use renoa_rcp_wire::TaskId;
use tokio::sync::oneshot;

use super::Chats;
use crate::{
    chat::Transcript,
    link::{Command, StopOutcome, StopProblem},
    theme::{Palette, u},
};

impl Chats {
    pub(super) fn composer_bar(
        &self,
        working: bool,
        palette: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let sendable = self.linked && !self.composer.read(cx).value().trim().is_empty();
        let running = if working {
            self.transcript().and_then(Transcript::running_command)
        } else {
            None
        };
        let stopping = running.is_some_and(|id| self.stopping.contains(&id));
        let stoppable = self.linked && running.is_some() && !stopping;
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);

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
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(u(6.))
                    .when(running.is_some(), |controls| {
                        controls.child(stop_button(stoppable, stopping, palette, cx))
                    })
                    .child(send_button(sendable, palette, cx)),
            )
    }

    /// Sends what the composer holds: with `steer` (Enter), into the agent's
    /// running turn, if any; without (Ctrl+Enter), as a turn of its own.
    pub(super) fn send(&mut self, steer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_owned();
        let Some(task_id) = self.chosen_task() else {
            return;
        };
        // Offline, the text stays in the composer until Renoa is back.
        if text.is_empty() || !self.linked {
            return;
        }
        let _ = self.commands.send(Command::Send {
            task_id,
            text,
            steer,
            sent: None,
        });
        self.unsent.remove(&task_id);
        self.stop_notices.remove(&task_id);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(task_id) = self.chosen_task() else {
            return;
        };
        let Some(command_id) = self.transcript().and_then(Transcript::running_command) else {
            return;
        };
        if !self.linked || self.stopping.contains(&command_id) {
            return;
        }
        self.stopping.insert(command_id);
        self.stop_notices.remove(&task_id);
        let (answered, rx) = oneshot::channel();
        if self
            .commands
            .send(Command::Stop {
                task_id,
                command_id,
                answered,
            })
            .is_err()
        {
            self.finish_stop(task_id, command_id, Err(StopProblem::Uncertain), cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let outcome = rx.await.unwrap_or(Err(StopProblem::Uncertain));
            let _ = this.update(cx, |this, cx| {
                this.finish_stop(task_id, command_id, outcome, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_stop(
        &mut self,
        task_id: TaskId,
        command_id: CommandId,
        outcome: Result<StopOutcome, StopProblem>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(StopOutcome::Requested) => {
                self.stop_notices.remove(&task_id);
            }
            Ok(StopOutcome::AlreadyEnded) => {
                self.stopping.remove(&command_id);
                self.stop_notices.remove(&task_id);
            }
            Err(StopProblem::Refused) => {
                self.stopping.remove(&command_id);
                self.stop_notices
                    .insert(task_id, "Couldn't stop that turn. Try again.");
            }
            Err(StopProblem::Uncertain) => {
                self.stopping.remove(&command_id);
                self.stop_notices.insert(
                    task_id,
                    "Renoa couldn't confirm the stop. It may still have gone through.",
                );
            }
        }
        cx.notify();
    }
}

/// The round stop button shown alongside Send while a turn is running.
fn stop_button(
    stoppable: bool,
    stopping: bool,
    palette: Palette,
    cx: &Context<Chats>,
) -> impl IntoElement + use<> {
    let (fill, glyph) = if stoppable {
        (palette.text, palette.background)
    } else if stopping {
        (palette.chat_agent, palette.secondary)
    } else {
        (palette.chat_agent, palette.secondary.opacity(0.6))
    };
    div()
        .id("stop")
        .size(u(32.))
        .flex_none()
        .rounded_full()
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .when(stoppable, |button| {
            button
                .cursor_pointer()
                .hover(|button| button.opacity(0.84))
                .on_click(cx.listener(|this: &mut Chats, _: &ClickEvent, _, cx| this.stop(cx)))
        })
        .test_support()
        .child(div().size(u(10.)).rounded(u(2.)).bg(glyph))
}

/// The round send button: ink when there is something to send, muted when not.
fn send_button(sendable: bool, palette: Palette, cx: &Context<Chats>) -> impl IntoElement + use<> {
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
        .on_click(cx.listener(|this: &mut Chats, _, window, cx| this.send(true, window, cx)))
        .test_support()
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
