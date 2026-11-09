//! The app's own settings: how it looks, the keys that move around it, this
//! Mac's sign-in, and which version it is.

use gpui_kit::{
    Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, assets::IconName,
    div, prelude::FluentBuilder as _, px,
};

use super::{Page, SheetEvent, group, header, pill, section};
use crate::theme::{self, Appearance, Palette};

/// The keys that move around the app, as Settings lists them. The bindings
/// themselves are `chats::keys`; a test checks each listed one is bound.
pub const SHORTCUTS: [(&str, &str); 7] = [
    ("Jump to an agent or page", "⌘K"),
    ("Open the first nine agents", "⌘1 – ⌘9"),
    ("Routines", "⌘⇧R"),
    ("Memory", "⌘⇧M"),
    ("Apps", "⌘⇧A"),
    ("Settings", "⌘,"),
    ("Close a sheet", "Esc"),
];

pub struct Settings {
    /// What this Mac calls itself, which Renoa shows on the phone.
    computer: SharedString,
    /// Whether the owner has asked once to sign out, and must click again.
    signing_out: bool,
    /// Whether the last choice could not be kept for the next launch.
    unkept: bool,
}

impl EventEmitter<SheetEvent> for Settings {}

impl Settings {
    pub fn new(computer: SharedString) -> Self {
        Self {
            computer,
            signing_out: false,
            unkept: false,
        }
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window, cx);
        let chosen = cx.try_global::<Appearance>().copied().unwrap_or_default();
        let top = header(
            Page::Settings,
            "How Renoa looks and works on this Mac.",
            palette,
            Vec::new(),
            cx.listener(|_, _, _, cx| cx.emit(SheetEvent::Close)),
        );
        let choice = |appearance: Appearance,
                      label: &'static str,
                      icon: Option<IconName>,
                      cx: &mut Context<Self>| {
            let on = chosen == appearance;
            div()
                .id(label)
                .flex_1()
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(on, |choice| {
                    choice.bg(palette.card).font_weight(FontWeight::SEMIBOLD)
                })
                .when(!on, |choice| choice.text_color(palette.secondary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    theme::apply(appearance, window, cx);
                    this.unkept = theme::keep_appearance(appearance).is_err();
                    cx.notify();
                }))
                .when_some(icon, |choice, icon| choice.child(icon))
                .child(label)
        };
        let looks = div()
            .p(px(3.))
            .flex()
            .gap(px(3.))
            .rounded(px(10.))
            .bg(palette.group)
            .child(choice(Appearance::System, "System", None, cx))
            .child(choice(Appearance::Light, "Light", Some(IconName::Sun), cx))
            .child(choice(Appearance::Dark, "Dark", Some(IconName::Moon), cx));
        let keys = SHORTCUTS.iter().map(|(what, keys)| {
            div()
                .flex()
                .justify_between()
                .px(px(14.))
                .py(px(9.))
                .child(*what)
                .child(
                    div()
                        .px(px(8.))
                        .rounded(px(6.))
                        .bg(palette.chat_agent)
                        .text_size(px(12.))
                        .text_color(palette.secondary)
                        .child(*keys),
                )
                .into_any_element()
        });
        let signing_out = self.signing_out;
        let this_mac = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(format!("Signed in as {}", self.computer))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if signing_out {
                                palette.danger
                            } else {
                                palette.secondary
                            })
                            .child(if signing_out {
                                "This Mac forgets its sign-in. Signing in again takes a new code from your phone."
                            } else {
                                "Approved from your phone"
                            }),
                    ),
            )
            .child(
                pill(
                    "sign-out",
                    if signing_out {
                        "Sign out now"
                    } else {
                        "Sign out of this Mac"
                    },
                    signing_out,
                    palette,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.signing_out {
                        cx.emit(SheetEvent::SignOut);
                    } else {
                        this.signing_out = true;
                        cx.notify();
                    }
                })),
            )
            .into_any_element();
        div().size_full().flex().flex_col().child(top).child(
            div()
                .id("settings")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(24.))
                .pb(px(24.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(section("Appearance", palette))
                .child(looks)
                .when(self.unkept, |body| {
                    body.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.attention)
                            .child(
                                "Renoa looks this way until it quits; it couldn't keep the choice.",
                            ),
                    )
                })
                .child(section("Keyboard", palette))
                .child(group(keys, palette))
                .child(section("This Mac", palette))
                .child(group([this_mac], palette))
                .child(section("Renoa", palette))
                .child(group(
                    [div()
                        .px(px(14.))
                        .py(px(12.))
                        .child(format!("Renoa for Mac {}", env!("CARGO_PKG_VERSION")))
                        .into_any_element()],
                    palette,
                )),
        )
    }
}
