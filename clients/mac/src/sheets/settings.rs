//! The app's own settings: how it looks, the keys that move around it, this
//! Mac's sign-in, and which version it is and whether a newer one is ready.

use gpui_kit::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    assets::IconName, div, prelude::FluentBuilder as _,
};

use super::{Page, SheetEvent, group, header, pill, section};
use crate::{
    theme::{self, Appearance, Palette, u},
    update::{self, Updater, Updates, updater::State},
};

/// The keys that move around the app, as Settings lists them. The keys
/// themselves are `chats::keys`; a test in `chats/tests.rs` presses each one
/// listed here and checks it leads where its words say.
pub const SHORTCUTS: [(&str, &str); 12] = [
    ("Jump to an agent or page", "⌘K"),
    ("Home", "⌘0"),
    ("Open the first nine agents", "⌘1 – ⌘9"),
    ("About the agent you're talking to", "⌘I"),
    ("Routines", "⌘⇧R"),
    ("Memory", "⌘⇧M"),
    ("Apps", "⌘⇧A"),
    ("Settings", "⌘,"),
    ("Close a sheet", "Esc"),
    ("Bigger", "⌘="),
    ("Smaller", "⌘−"),
    ("Actual size", "⌥⌘0"),
];

pub struct Settings {
    /// What this Mac calls itself, which Renoa shows on the phone.
    computer: SharedString,
    /// Whether the owner has asked once to sign out, and must click again.
    signing_out: bool,
    /// Whether the last choice could not be kept for the next launch.
    unkept: bool,
    updates: Option<Entity<Updater>>,
}

impl EventEmitter<SheetEvent> for Settings {}

impl Settings {
    /// Settings, checking again for an update unless one is already found.
    pub fn new(computer: SharedString, cx: &mut Context<Self>) -> Self {
        let updates = cx.try_global::<Updates>().map(|updates| updates.0.clone());
        if let Some(updates) = &updates {
            cx.observe(updates, |_, _, cx| cx.notify()).detach();
            updates.update(cx, |updater, cx| {
                if matches!(updater.state(), State::Current | State::CheckFailed(_)) {
                    updater.check(cx);
                }
            });
        }
        Self {
            computer,
            signing_out: false,
            unkept: false,
            updates,
        }
    }
}

/// Which version this is, and its update.
fn version_row(
    updates: Option<&Entity<Updater>>,
    palette: Palette,
    cx: &gpui_kit::App,
) -> AnyElement {
    let state = updates.map_or(State::FromSource, |updates| {
        updates.read(cx).state().clone()
    });
    let (said, problem, action) = match &state {
        State::FromSource => (
            "Built from source; it doesn't update itself.".to_owned(),
            false,
            None,
        ),
        State::Checking => ("Checking for updates…".to_owned(), false, None),
        State::Current => ("Up to date".to_owned(), false, Some(("Check now", false))),
        State::CheckFailed(_) => (
            "Couldn't check for updates.".to_owned(),
            true,
            Some(("Check now", false)),
        ),
        State::Available(release) => (
            format!("Renoa {} is ready.", release.version_name),
            false,
            Some(("Update and relaunch", true)),
        ),
        State::Installing(release) => (
            format!("Updating to Renoa {}…", release.version_name),
            false,
            None,
        ),
        State::InstallFailed(_, reason) => (
            format!("Couldn't update: {reason}"),
            true,
            Some(("Try again", true)),
        ),
        State::Updated(release, reason) => (update::updater::updated(release, reason), true, None),
    };
    let updates = updates.cloned();
    div()
        .flex()
        .items_center()
        .gap(u(12.))
        .px(u(14.))
        .py(u(12.))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(u(2.))
                .child(format!(
                    "Renoa for Mac {}",
                    update::VERSION.unwrap_or(env!("CARGO_PKG_VERSION"))
                ))
                .child(
                    div()
                        .text_size(u(12.))
                        .text_color(if problem {
                            palette.attention
                        } else {
                            palette.secondary
                        })
                        .child(said),
                ),
        )
        .children(action.zip(updates).map(|((label, filled), updates)| {
            pill("update", label, filled, palette).on_click(move |_, _, cx| {
                updates.update(cx, |updater, cx| {
                    if filled {
                        updater.install(cx);
                    } else {
                        updater.check(cx);
                    }
                });
            })
        }))
        .into_any_element()
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
                .h(u(30.))
                .flex()
                .items_center()
                .justify_center()
                .gap(u(6.))
                .rounded(u(8.))
                .cursor_pointer()
                .when(on, |choice| {
                    choice
                        .bg(palette.surface_raised)
                        .border_1()
                        .border_color(palette.outline)
                        .shadow_2xs()
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!on, |choice| {
                    choice
                        .text_color(palette.secondary)
                        .hover(|choice| choice.text_color(palette.text))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    theme::apply(appearance, window, cx);
                    this.unkept = theme::keep_appearance(appearance).is_err();
                    cx.notify();
                }))
                .when_some(icon, |choice, icon| choice.child(icon))
                .child(label)
        };
        let looks = div()
            .flex_none()
            .p(u(3.))
            .flex()
            .gap(u(3.))
            .rounded(u(10.))
            .bg(palette.group)
            .border_1()
            .border_color(palette.outline)
            .child(choice(Appearance::System, "System", None, cx))
            .child(choice(Appearance::Light, "Light", Some(IconName::Sun), cx))
            .child(choice(Appearance::Dark, "Dark", Some(IconName::Moon), cx));
        let keys = SHORTCUTS.iter().map(|(what, keys)| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(u(14.))
                .py(u(9.))
                .child(*what)
                .child(
                    div()
                        .px(u(8.))
                        .py(u(2.))
                        .rounded(u(6.))
                        .bg(palette.chat_agent)
                        .border_1()
                        .border_color(palette.outline)
                        .text_size(u(11.5))
                        .text_color(palette.secondary)
                        .child(*keys),
                )
                .into_any_element()
        });
        let signing_out = self.signing_out;
        let this_mac = div()
            .flex()
            .items_center()
            .gap(u(12.))
            .px(u(14.))
            .py(u(12.))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(u(2.))
                    .child(format!("Signed in as {}", self.computer))
                    .child(
                        div()
                            .text_size(u(12.))
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
            .when(signing_out, |row| {
                row.child(pill("keep", "Cancel", false, palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.signing_out = false;
                        cx.notify();
                    },
                )))
            })
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
                .px(u(24.))
                .pb(u(24.))
                .flex()
                .flex_col()
                .gap(u(6.))
                .child(section("Appearance", palette))
                .child(looks)
                .when(self.unkept, |body| {
                    body.child(
                        div().text_size(u(12.)).text_color(palette.attention).child(
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
                    [version_row(self.updates.as_ref(), palette, cx)],
                    palette,
                )),
        )
    }
}
