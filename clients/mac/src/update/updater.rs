//! The app's update, as the window and Settings show it: checked once at
//! launch and whenever Settings opens, installed when the owner asks.

use gpui_kit::{
    AnyElement, Context, Entity, Global, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Task, div,
};

use super::{Release, VERSION, description_url, install, newer, relaunch, running_bundle};
use crate::{
    link,
    sheets::pill,
    theme::{Palette, u},
};

/// Where the app's update stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// This build does not update itself: it was not published, or it does
    /// not run from Renoa.app.
    FromSource,
    Checking,
    Current,
    CheckFailed(String),
    Available(Release),
    Installing(Release),
    InstallFailed(Release, String),
}

pub struct Updater {
    url: String,
    client: reqwest::Client,
    state: State,
    _work: Option<Task<()>>,
}

/// The one updater, for Settings to read.
pub struct Updates(pub Entity<Updater>);

impl Global for Updates {}

impl Updater {
    /// The updater for the app served at `origin`, already checking.
    pub fn new(origin: &str, cx: &mut Context<Self>) -> Self {
        let mut updater = Self {
            url: description_url(origin),
            client: reqwest::Client::new(),
            state: State::FromSource,
            _work: None,
        };
        updater.check(cx);
        updater
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Asks renoa.live for a newer app, unless this build does not update or
    /// is already checking or installing.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let (Some(running), Some(_)) = (VERSION, running_bundle()) else {
            return;
        };
        if matches!(self.state, State::Checking | State::Installing(_)) {
            return;
        }
        self.state = State::Checking;
        cx.notify();
        let (client, url) = (self.client.clone(), self.url.clone());
        let checked = link::runtime().spawn(async move { newer(&client, &url, running).await });
        self._work = Some(cx.spawn(async move |this, cx| {
            let found = checked.await.unwrap_or_else(|error| Err(error.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.state = match found {
                    Ok(Some(release)) => State::Available(release),
                    Ok(None) => State::Current,
                    Err(reason) => {
                        eprintln!("renoa: {reason}");
                        State::CheckFailed(reason)
                    }
                };
                cx.notify();
            });
        }));
    }

    /// Puts the newer app in place of this one and reopens it.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let (State::Available(release) | State::InstallFailed(release, _)) = &self.state else {
            return;
        };
        let Some(bundle) = running_bundle() else {
            return;
        };
        let release = release.clone();
        self.state = State::Installing(release.clone());
        cx.notify();
        let (client, url) = (self.client.clone(), self.url.clone());
        let installed = link::runtime().spawn({
            let (release, bundle) = (release.clone(), bundle.clone());
            async move { install(&client, &url, &release, &bundle).await }
        });
        self._work = Some(cx.spawn(async move |this, cx| {
            let done = installed
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                .and_then(|()| {
                    relaunch(&bundle).map_err(|error| {
                        format!("Renoa updated but couldn't reopen ({error}); open it again")
                    })
                });
            match done {
                Ok(()) => cx.update(|cx| cx.quit()),
                Err(reason) => {
                    eprintln!("renoa: {reason}");
                    let _ = this.update(cx, |this, cx| {
                        this.state = State::InstallFailed(release, reason);
                        cx.notify();
                    });
                }
            }
        }));
    }
}

/// The bar along the bottom of the window while a newer app is ready, being
/// installed, or failed to install.
pub fn bar(updates: &Entity<Updater>, palette: Palette, cx: &gpui_kit::App) -> Option<AnyElement> {
    let (said, action) = match updates.read(cx).state() {
        State::Available(release) => (
            format!("Renoa {} is ready.", release.version_name),
            Some("Update and relaunch"),
        ),
        State::Installing(release) => {
            (format!("Updating to Renoa {}…", release.version_name), None)
        }
        State::InstallFailed(_, reason) => {
            (format!("Couldn't update: {reason}"), Some("Try again"))
        }
        _ => return None,
    };
    let updates = updates.clone();
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(u(12.))
            .px(u(16.))
            .py(u(8.))
            .border_t_1()
            .border_color(palette.divider)
            .bg(palette.group)
            .text_size(u(13.))
            .text_color(palette.text)
            .child(div().flex_1().min_w_0().child(said))
            .children(action.map(|action| {
                pill("update", action, true, palette).on_click(move |_, _, cx| {
                    updates.update(cx, |updater, cx| updater.install(cx));
                })
            }))
            .into_any_element(),
    )
}
