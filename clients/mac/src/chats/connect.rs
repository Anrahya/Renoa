//! Setup link cards waiting for the owner, shown over whatever screen is open
//! so a link from any agent is always reachable. Connect opens the link in the
//! system browser and acknowledges it; Not now acknowledges it without opening.

use std::collections::HashSet;

use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, div,
    prelude::FluentBuilder as _,
};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::Chats;
use crate::{
    link::Command,
    notice::{HeldLink, now_ms},
    sheets::{mark, pill},
    theme::{Palette, u},
};

const ACK_FAILED: &str = "Could not reach Renoa. Check your connection and try again.";

/// Held setup links and any in-flight or failed acknowledgement for them.
#[derive(Default)]
pub(super) struct SetupLinks {
    links: Vec<HeldLink>,
    pending: HashSet<Uuid>,
    problem: Option<(Uuid, &'static str)>,
}

impl SetupLinks {
    pub(super) fn set(&mut self, links: Vec<HeldLink>) {
        self.links = links;
        self.pending
            .retain(|id| self.links.iter().any(|item| item.notice_id == *id));
        if let Some((id, _)) = self.problem
            && !self.links.iter().any(|item| item.notice_id == id)
        {
            self.problem = None;
        }
    }

    pub(super) fn current(&self) -> Option<(&HeldLink, usize)> {
        let now = now_ms();
        let active: Vec<&HeldLink> = self
            .links
            .iter()
            .filter(|item| item.link.expires_at_ms > now)
            .collect();
        let count = active.len();
        active.first().copied().map(|first| (first, count))
    }

    #[cfg(test)]
    pub(super) fn problem_for(&self, notice_id: Uuid) -> Option<&'static str> {
        self.problem
            .filter(|(id, _)| *id == notice_id)
            .map(|(_, text)| text)
    }
}

impl Chats {
    /// The first setup link waiting for the owner, floating at the top of the
    /// workspace.
    pub(super) fn setup_link_card(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let (held, count) = self.setup_links.current()?;
        let notice_id = held.notice_id;
        let url = held.link.url.clone();
        let expires_at_ms = held.link.expires_at_ms;
        let label = held.link.label.clone();
        let words = held.link.kind.words();
        let host = held.link.host();
        let problem = self
            .setup_links
            .problem
            .filter(|(id, _)| *id == notice_id)
            .map(|(_, text)| text);
        let busy = self.setup_links.pending.contains(&notice_id);

        Some(
            div()
                .absolute()
                .top(u(14.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .px(u(16.))
                .child(
                    div()
                        .w_full()
                        .max_w(u(420.))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .flex()
                        .flex_col()
                        .gap(u(12.))
                        .px(u(16.))
                        .py(u(14.))
                        .rounded(u(16.))
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.outline_strong)
                        .shadow_xl()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(u(12.))
                                .child(mark(&label, palette))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(u(2.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(u(8.))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(palette.text)
                                                        .child(label),
                                                )
                                                .when(count > 1, |row| {
                                                    row.child(
                                                        div()
                                                            .flex_none()
                                                            .text_size(u(11.))
                                                            .text_color(palette.secondary)
                                                            .child(format!("1 of {count}")),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(u(12.5))
                                                .text_color(palette.secondary)
                                                .child(words),
                                        )
                                        .when_some(host, |col, host| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_size(u(11.5))
                                                    .text_color(palette.secondary)
                                                    .child(format!("Opens {host}")),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(u(8.))
                                .child(
                                    pill("setup-link-dismiss", "Not now", false, palette)
                                        .test_support()
                                        .when(busy, |button| button.opacity(0.6))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.acknowledge_setup_link(notice_id, cx);
                                        })),
                                )
                                .child(
                                    pill("setup-link-connect", "Connect", true, palette)
                                        .test_support()
                                        .when(busy, |button| button.opacity(0.6))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_setup_link(
                                                notice_id,
                                                &url,
                                                expires_at_ms,
                                                cx,
                                            );
                                        })),
                                ),
                        )
                        .when_some(problem, |card, text| {
                            card.child(
                                div()
                                    .text_size(u(12.))
                                    .text_color(palette.danger)
                                    .child(text),
                            )
                        }),
                ),
        )
    }

    pub(super) fn open_setup_link(
        &mut self,
        notice_id: Uuid,
        url: &str,
        expires_at_ms: i64,
        cx: &mut Context<Self>,
    ) {
        if self.setup_links.pending.contains(&notice_id) {
            return;
        }
        // A link that expired while on screen leads nowhere; put it away instead.
        if expires_at_ms > now_ms() {
            cx.open_url(url);
        }
        self.acknowledge_setup_link(notice_id, cx);
    }

    pub(super) fn acknowledge_setup_link(&mut self, notice_id: Uuid, cx: &mut Context<Self>) {
        if !self.setup_links.pending.insert(notice_id) {
            return;
        }
        if self
            .setup_links
            .problem
            .is_some_and(|(id, _)| id == notice_id)
        {
            self.setup_links.problem = None;
        }
        let (tx, rx) = oneshot::channel();
        let sent = self
            .commands
            .send(Command::AcknowledgeNotice {
                notice_id,
                answered: tx,
            })
            .is_ok();
        cx.notify();
        if !sent {
            self.setup_links.pending.remove(&notice_id);
            self.setup_links.problem = Some((notice_id, ACK_FAILED));
            return;
        }
        cx.spawn(async move |this, cx| {
            let ok = rx.await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.setup_links.pending.remove(&notice_id);
                if ok {
                    this.setup_links
                        .links
                        .retain(|item| item.notice_id != notice_id);
                    if this
                        .setup_links
                        .problem
                        .is_some_and(|(id, _)| id == notice_id)
                    {
                        this.setup_links.problem = None;
                    }
                } else {
                    this.setup_links.problem = Some((notice_id, ACK_FAILED));
                }
                cx.notify();
            });
        })
        .detach();
    }
}
