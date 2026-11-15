//! The keys that move around the app. The ones the menus show are bound to
//! actions (`bind`), so a menu item and its key are one; the rest are read
//! as they are pressed (`shortcut`). Settings lists them all
//! (`sheets::settings::SHORTCUTS`); a test in `chats/tests.rs` presses each
//! one it lists and checks where it leads.

use gpui_kit::{
    Action, App, Context, Div, InteractiveElement as _, KeyBinding, Keystroke,
    ModifiersChangedEvent, actions,
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

/// `keys` held with the modifier the app's keys use, ⌘ on macOS and Ctrl
/// elsewhere, as the `&'static str` a key binding takes: `command!("shift-r")`.
macro_rules! command {
    ($keys:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $keys)
        } else {
            concat!("ctrl-", $keys)
        }
    };
}
pub(crate) use command;

/// Binds the keys the menus show to their actions, for the whole app.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(command!("0"), GoHome, None),
        KeyBinding::new(command!("k"), JumpTo, None),
        KeyBinding::new(command!("i"), AboutAgent, None),
        KeyBinding::new(command!("shift-r"), OpenRoutines, None),
        KeyBinding::new(command!("shift-m"), OpenMemory, None),
        KeyBinding::new(command!("shift-a"), OpenApps, None),
        KeyBinding::new(command!(","), OpenSettings, None),
        KeyBinding::new(command!("b"), ToggleSidebar, None),
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
    /// Goes to the chat opened before, or back up the list with Shift, while
    /// Ctrl stays held.
    Cycle {
        back: bool,
    },
}

/// The move an unbound `keystroke` asks for, if any: Esc, which the text
/// fields would otherwise take, the agents' numbers, and Ctrl+Tab, which is
/// Ctrl on every platform, as in a browser.
pub fn shortcut(keystroke: &Keystroke) -> Option<Move> {
    let held = &keystroke.modifiers;
    if keystroke.key == "tab" && held.control && !held.platform && !held.alt {
        return Some(Move::Cycle { back: held.shift });
    }
    // The modifier `command` binds with.
    let command = if cfg!(target_os = "macos") {
        held.platform && !held.control
    } else {
        held.control && !held.platform
    } && !held.alt;
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

/// `keys`, written with the Mac's symbols as Settings and the menus list
/// them, the way this platform writes them: as they are on macOS, and as
/// "Ctrl+Shift+R" elsewhere.
pub fn shown(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return keys.to_owned();
    }
    keys.chars()
        .map(|key| match key {
            '⌘' | '⌃' => "Ctrl+".to_owned(),
            '⇧' => "Shift+".to_owned(),
            '⌥' => "Alt+".to_owned(),
            other => other.to_string(),
        })
        .collect()
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
        // Ctrl coming up ends a Ctrl+Tab walk on the chat it reached.
        root.on_modifiers_changed(cx.listener(|chats, event: &ModifiersChangedEvent, _, _| {
            if !event.modifiers.control {
                chats.recent.settle();
            }
        }))
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

    use super::{Move, shortcut, shown};

    fn press(keys: &'static str) -> Option<Move> {
        shortcut(&Keystroke::parse(keys).expect("a keystroke"))
    }

    #[test]
    fn typing_is_left_alone() {
        for keys in [
            "k",
            "shift-r",
            // ⌘ and Ctrl held together.
            "cmd-ctrl-1",
            // The other platform's modifier alone.
            if cfg!(target_os = "macos") {
                "ctrl-1"
            } else {
                "cmd-1"
            },
            command!("alt-1"),
            command!("shift-escape"),
            "tab",
            "alt-ctrl-tab",
        ] {
            assert_eq!(press(keys), None, "{keys}");
        }
    }

    #[test]
    fn ctrl_tab_cycles_on_every_platform() {
        assert_eq!(press("ctrl-tab"), Some(Move::Cycle { back: false }));
        assert_eq!(press("ctrl-shift-tab"), Some(Move::Cycle { back: true }));
    }

    #[test]
    fn keys_are_written_the_way_the_platform_writes_them() {
        let written = ["⌘K", "⌘⇧R", "⌥⌘0", "⌘1 – ⌘9", "Esc", "⌃Tab"].map(shown);
        if cfg!(target_os = "macos") {
            assert_eq!(written, ["⌘K", "⌘⇧R", "⌥⌘0", "⌘1 – ⌘9", "Esc", "⌃Tab"]);
        } else {
            assert_eq!(
                written,
                [
                    "Ctrl+K",
                    "Ctrl+Shift+R",
                    "Alt+Ctrl+0",
                    "Ctrl+1 – Ctrl+9",
                    "Esc",
                    "Ctrl+Tab"
                ]
            );
        }
    }
}
