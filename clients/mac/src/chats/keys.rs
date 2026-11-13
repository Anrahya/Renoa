//! The keys that move around the app. The ones the menus show are bound to
//! actions (`bind`), so a menu item and its key are one; the rest are read
//! as they are pressed (`shortcut`). Settings lists them all
//! (`sheets::settings::SHORTCUTS`); a test in `chats/tests.rs` presses each
//! one it lists and checks where it leads.

use gpui_kit::{
    Action, App, Context, Div, InteractiveElement as _, KeyBinding, Keystroke, actions,
};

use super::Chats;

use crate::sheets::Page;

actions!(
    renoa,
    [
        GoHome,
        JumpTo,
        AboutAgent,
        OpenRoutines,
        OpenMemory,
        OpenApps,
        OpenSettings,
        ToggleSidebar
    ]
);

/// Binds the keys the menus show to their actions, for the whole app.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-0", GoHome, None),
        KeyBinding::new("cmd-k", JumpTo, None),
        KeyBinding::new("cmd-i", AboutAgent, None),
        KeyBinding::new("cmd-shift-r", OpenRoutines, None),
        KeyBinding::new("cmd-shift-m", OpenMemory, None),
        KeyBinding::new("cmd-shift-a", OpenApps, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
    ]);
}

/// Where a key takes the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Opens the box that jumps to an agent or a page.
    Jump,
    /// Opens the chat with the agent at this place in the list, from 0.
    Agent(usize),
    Home,
    Sheet(Page),
    /// Opens the page of the agent whose chat is open.
    Profile,
    /// Closes the sheet or the jump box that is up.
    Close,
    /// Shows or hides the sidebar.
    ToggleSidebar,
}

/// The move an unbound `keystroke` asks for, if any: Esc, which the text
/// fields would otherwise take, and the agents' numbers.
pub fn shortcut(keystroke: &Keystroke) -> Option<Move> {
    let held = &keystroke.modifiers;
    let command = held.platform && !held.control && !held.alt;
    let key = keystroke.key.as_str();
    match (command, held.shift, key) {
        (false, false, "escape") => Some(Move::Close),
        (true, false, digit) => match digit.parse::<usize>() {
            Ok(place @ 1..=9) => Some(Move::Agent(place - 1)),
            _ => None,
        },
        _ => None,
    }
}

/// Answers each bound action with its move. ⌘I is answered only while a
/// page can open, so elsewhere its menu item is dimmed and the key passes.
impl Chats {
    pub(super) fn answer_keys(&self, root: Div, cx: &mut Context<Self>) -> Div {
        let mut root = root;
        let page = self.profile_task().is_some();
        for (moved, listen) in [
            (Move::Home, listen::<GoHome> as Listen),
            (Move::Jump, listen::<JumpTo>),
            (Move::Profile, listen::<AboutAgent>),
            (Move::Sheet(Page::Routines), listen::<OpenRoutines>),
            (Move::Sheet(Page::Memory), listen::<OpenMemory>),
            (Move::Sheet(Page::Apps), listen::<OpenApps>),
            (Move::Sheet(Page::Settings), listen::<OpenSettings>),
            (Move::ToggleSidebar, listen::<ToggleSidebar>),
        ] {
            if moved != Move::Profile || page {
                root = listen(root, moved, cx);
            }
        }
        root
    }
}

type Listen = fn(Div, Move, &mut Context<Chats>) -> Div;

fn listen<A: Action>(root: Div, moved: Move, cx: &mut Context<Chats>) -> Div {
    root.on_action(cx.listener(move |chats, _: &A, window, cx| {
        chats.go(moved, window, cx);
    }))
}

#[cfg(test)]
mod tests {
    use gpui_kit::Keystroke;

    use super::{Move, shortcut};

    fn press(keys: &'static str) -> Option<Move> {
        shortcut(&Keystroke::parse(keys).expect("a keystroke"))
    }

    #[test]
    fn typing_is_left_alone() {
        for keys in [
            "k",
            "shift-r",
            "cmd-ctrl-k",
            "cmd-alt-1",
            "cmd-shift-escape",
        ] {
            assert_eq!(press(keys), None, "{keys}");
        }
    }
}
