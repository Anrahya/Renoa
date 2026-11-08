//! The window's root: signing in until this Mac has a credential, then the
//! chats.

use std::{process::Command, sync::Arc};

use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window, div, prelude::FluentBuilder as _, px,
};
use renoa_rcp_wire::{DeviceCode, DeviceCredentials};
use tokio::sync::mpsc;

use crate::{
    chats::{Chats, SignedOut},
    key::MacKey,
    link::{self, Endpoints, SignInStep},
    theme::Palette,
    vault,
};

/// The longest name a device may carry.
const MAX_LABEL_CHARS: usize = 64;

/// What the owner reads when the Keychain fails; the detail goes to the log.
const KEYCHAIN_PROBLEM: &str =
    "Renoa couldn't use this Mac's Keychain. Unlock it in Keychain Access, then open Renoa again.";

enum Screen {
    /// The Keychain cannot keep what Renoa needs; nothing else can work.
    Problem,
    SigningIn {
        code: Option<DeviceCode>,
        unreachable: bool,
    },
    Chats(Entity<Chats>),
}

pub struct Renoa {
    endpoints: Endpoints,
    key: Option<Arc<MacKey>>,
    screen: Screen,
    _work: Option<Task<()>>,
}

impl Renoa {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut renoa = Self {
            endpoints: Endpoints::from_environment(),
            key: None,
            screen: Screen::SigningIn {
                code: None,
                unreachable: false,
            },
            _work: None,
        };
        match vault::key() {
            Err(problem) => renoa.fail(&problem),
            Ok(key) => {
                renoa.key = Some(Arc::new(key));
                match vault::credentials() {
                    Ok(Some(credentials)) => renoa.open_chats(credentials, window, cx),
                    Ok(None) => renoa.sign_in(window, cx),
                    Err(problem) => renoa.fail(&problem),
                }
            }
        }
        renoa
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else { return };
        self.screen = Screen::SigningIn {
            code: None,
            unreachable: false,
        };
        let (steps_sender, mut steps) = mpsc::unbounded_channel();
        link::runtime().spawn(link::sign_in(
            self.endpoints.clone(),
            key,
            computer_name(),
            link::Pace::APP,
            vault::keep_credentials,
            steps_sender,
        ));
        self._work = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(step) = steps.recv().await {
                let applied = this.update_in(cx, |this, window, cx| this.step(step, window, cx));
                if applied.is_err() {
                    return;
                }
            }
        }));
    }

    fn step(&mut self, step: SignInStep, window: &mut Window, cx: &mut Context<Self>) {
        match step {
            SignInStep::Showing(code) => {
                self.screen = Screen::SigningIn {
                    code: Some(code),
                    unreachable: false,
                };
            }
            SignInStep::Reachable(reachable) => {
                if let Screen::SigningIn { unreachable, .. } = &mut self.screen {
                    *unreachable = !reachable;
                }
            }
            SignInStep::SignedIn(credentials) => self.open_chats(credentials, window, cx),
            SignInStep::NotKept(problem) => self.fail(&problem),
        }
        cx.notify();
    }

    fn fail(&mut self, problem: &str) {
        eprintln!("renoa: {problem}");
        self._work = None;
        self.screen = Screen::Problem;
    }

    fn open_chats(
        &mut self,
        credentials: DeviceCredentials,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.key.clone() else { return };
        let (commands, commands_received) = mpsc::unbounded_channel();
        let (updates_sender, updates) = mpsc::unbounded_channel();
        link::runtime().spawn(link::session(
            self.endpoints.clone(),
            credentials,
            key,
            link::Pace::APP,
            commands_received,
            updates_sender,
        ));
        let chats = cx.new(|cx| Chats::new(commands, updates, window, cx));
        cx.subscribe_in(&chats, window, |this, _, _: &SignedOut, window, cx| {
            vault::forget_credentials();
            this.sign_in(window, cx);
            cx.notify();
        })
        .detach();
        self._work = None;
        self.screen = Screen::Chats(chats);
    }
}

impl Render for Renoa {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(window);
        match &self.screen {
            Screen::Chats(chats) => div().size_full().child(chats.clone()),
            Screen::Problem => centered(palette)
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Renoa can't start"),
                )
                .child(
                    div()
                        .max_w(px(340.))
                        .text_center()
                        .text_color(palette.secondary)
                        .child(KEYCHAIN_PROBLEM),
                ),
            Screen::SigningIn { code, unreachable } => {
                let shown: SharedString = code.as_ref().map_or_else(
                    || "· · · ·  · · · ·".into(),
                    |code| spaced(code.expose()).into(),
                );
                centered(palette)
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Sign in to Renoa"),
                    )
                    .child(
                        div()
                            .my(px(8.))
                            .px(px(22.))
                            .py(px(14.))
                            .rounded(px(16.))
                            .bg(palette.chat_agent)
                            .font_family("SF Mono")
                            .text_size(px(34.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(shown),
                    )
                    .child(
                        div()
                            .max_w(px(340.))
                            .text_center()
                            .text_color(palette.secondary)
                            .child("On your phone, open Renoa, go to Settings, tap Sign in a computer, and type this code."),
                    )
                    .when(*unreachable, |screen| {
                        screen.child(
                            div()
                                .text_color(palette.attention)
                                .child("Can't reach Renoa. Trying again…"),
                        )
                    })
            }
        }
    }
}

fn centered(palette: Palette) -> gpui_kit::Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .bg(palette.background)
        .text_color(palette.text)
        .text_size(px(13.))
}

/// The code as two groups of four, the way it is easiest to read aloud.
fn spaced(code: &str) -> String {
    let (first, second) = code.split_at(code.len().min(4));
    format!("{first} {second}")
}

/// The name this Mac signs in with: its name in System Settings.
fn computer_name() -> String {
    let named = Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| {
            name.trim()
                .chars()
                .take(MAX_LABEL_CHARS)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty());
    named.unwrap_or_else(|| "Mac".to_owned())
}
