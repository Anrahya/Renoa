//! The keys that move around the app. Settings lists them
//! (`sheets::settings::SHORTCUTS`); a test checks that each key it lists
//! does what it says and that each sheet's own keys are among them.

use gpui_kit::Keystroke;

use crate::sheets::Page;

/// Where a key takes the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Opens the box that jumps to an agent or a page.
    Jump,
    /// Opens the chat with the agent at this place in the list, from 0.
    Agent(usize),
    Sheet(Page),
    /// Closes the sheet or the jump box that is up.
    Close,
}

/// The move `keystroke` asks for, if any.
pub fn shortcut(keystroke: &Keystroke) -> Option<Move> {
    let held = &keystroke.modifiers;
    let command = held.platform && !held.control && !held.alt;
    let key = keystroke.key.as_str();
    match (command, held.shift, key) {
        (false, false, "escape") => Some(Move::Close),
        (true, false, "k") => Some(Move::Jump),
        (true, false, ",") => Some(Move::Sheet(Page::Settings)),
        (true, true, "r") => Some(Move::Sheet(Page::Routines)),
        (true, true, "m") => Some(Move::Sheet(Page::Memory)),
        (true, true, "a") => Some(Move::Sheet(Page::Apps)),
        (true, false, digit) => match digit.parse::<usize>() {
            Ok(place @ 1..=9) => Some(Move::Agent(place - 1)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::Keystroke;

    use super::{Move, shortcut};
    use crate::sheets::{Page, settings::SHORTCUTS};

    fn press(keys: &'static str) -> Option<Move> {
        shortcut(&Keystroke::parse(keys).expect("a keystroke"))
    }

    #[test]
    fn each_key_settings_lists_does_what_it_says() {
        let pressed = |keys: &str| match keys {
            "⌘K" => press("cmd-k"),
            "⌘1 – ⌘9" => press("cmd-1").filter(|_| press("cmd-9") == Some(Move::Agent(8))),
            "⌘⇧R" => press("cmd-shift-r"),
            "⌘⇧M" => press("cmd-shift-m"),
            "⌘⇧A" => press("cmd-shift-a"),
            "⌘," => press("cmd-,"),
            "Esc" => press("escape"),
            other => panic!("Settings lists {other}, which this test does not press"),
        };
        for (what, keys) in SHORTCUTS {
            let moved = pressed(keys);
            let expected = match what {
                "Jump to an agent or page" => Move::Jump,
                "Open the first nine agents" => Move::Agent(0),
                "Close a sheet" => Move::Close,
                page => Move::Sheet(
                    Page::ALL
                        .into_iter()
                        .find(|listed| listed.title() == page)
                        .unwrap_or_else(|| panic!("no page named {page}")),
                ),
            };
            assert_eq!(moved, Some(expected), "{what}: {keys}");
        }
        for page in Page::ALL {
            assert!(
                SHORTCUTS
                    .iter()
                    .any(|(what, keys)| *what == page.title() && *keys == page.keys()),
                "{} shows its keys in Settings",
                page.title()
            );
        }
    }

    #[test]
    fn typing_is_left_alone() {
        for keys in [
            "k",
            "shift-r",
            "cmd-0",
            "cmd-ctrl-k",
            "cmd-alt-1",
            "cmd-shift-escape",
        ] {
            assert_eq!(press(keys), None, "{keys}");
        }
    }
}
