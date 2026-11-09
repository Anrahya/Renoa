//! The menu bar: Renoa's own menu, Edit for the text fields, Go for the
//! places the keys reach, and Window. Each item shows the keys bound to its
//! action, so the menus and the keys cannot disagree.

use gpui_kit::{
    App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions, component::input,
};

use crate::chats::keys::{
    self, AboutAgent, GoHome, JumpTo, OpenApps, OpenMemory, OpenRoutines, OpenSettings,
};

actions!(renoa, [Quit, Hide, HideOthers, ShowAll, Minimize, Zoom]);

/// Binds the keys, answers the app's own items and shows the menus.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    keys::bind(cx);
    cx.set_menus(menus());
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Renoa").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Renoa", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Renoa", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Home", GoHome),
            MenuItem::action("Jump To…", JumpTo),
            MenuItem::action("About This Agent", AboutAgent),
            MenuItem::separator(),
            MenuItem::action("Routines", OpenRoutines),
            MenuItem::action("Memory", OpenMemory),
            MenuItem::action("Apps", OpenApps),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use gpui_kit::{MenuItem, TestAppContext};

    use super::{install, menus};

    #[gpui_kit::test]
    fn each_menu_item_shows_its_keys_but_the_two_mac_apps_leave_bare(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
            let mut unbound = Vec::new();
            for menu in menus() {
                for item in menu.items {
                    if let MenuItem::Action { name, action, .. } = item
                        && cx
                            .key_bindings()
                            .borrow()
                            .bindings_for_action(action.as_ref())
                            .next()
                            .is_none()
                    {
                        unbound.push(name.to_string());
                    }
                }
            }
            assert_eq!(
                unbound,
                ["Show All", "Zoom"],
                "the text fields bind their own edits"
            );
        });
    }
}
