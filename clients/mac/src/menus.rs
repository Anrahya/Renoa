//! The menu bar: Renoa's own menu, Edit for the text fields, Go for the
//! places the keys reach, and Window. An item and its key are one action,
//! and the menu shows the keys bound to it.

use gpui_kit::{
    App, Div, InteractiveElement as _, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType,
    actions, component::input,
};

use crate::chats::keys::{
    self, AboutAgent, GoHome, JumpTo, OpenApps, OpenMemory, OpenRoutines, OpenSettings,
};

actions!(
    renoa,
    [Quit, Hide, HideOthers, ShowAll, Minimize, Zoom, CloseWindow]
);

/// Answers the Window items on the window `root` fills: they act on that
/// window, which an app-wide listener cannot reach while it dispatches.
pub fn window_items(root: Div) -> Div {
    root.on_action(|_: &Minimize, window, _| window.minimize_window())
        .on_action(|_: &Zoom, window, _| window.zoom_window())
        .on_action(|_: &CloseWindow, window, _| window.remove_window())
}

/// Binds the keys, answers the app's own items and shows the menus.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
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
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext as _, InteractiveElement as _, MenuItem, Styled as _, TestAppContext,
        test::TestWindowExt as _,
    };

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

    /// A window filled with the Window items, ready for its keys.
    fn window(cx: &mut TestAppContext) -> gpui_kit::AnyWindowHandle {
        struct Root(gpui_kit::FocusHandle);
        impl gpui_kit::Render for Root {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                super::window_items(gpui_kit::div().size_full().track_focus(&self.0))
            }
        }
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
        });
        let window = cx.add_window(|_, cx| Root(cx.focus_handle()));
        let focus = window.update(cx, |root, _, _| root.0.clone()).unwrap();
        let window: gpui_kit::AnyWindowHandle = window.into();
        cx.update_window(window, |_, window, cx| {
            window.focus(&focus, cx);
            window.render_frame(cx);
        })
        .unwrap();
        window
    }

    // The test window answers a minimize or a zoom by panicking as not
    // implemented: reaching it is what these check.
    #[gpui_kit::test]
    #[should_panic(expected = "not implemented")]
    fn minimize_reaches_the_window(cx: &mut TestAppContext) {
        let window = window(cx);
        cx.update_window(window, |_, window, cx| window.press("cmd-m", cx))
            .unwrap();
    }

    #[gpui_kit::test]
    #[should_panic(expected = "not implemented")]
    fn zoom_reaches_the_window(cx: &mut TestAppContext) {
        let window = window(cx);
        cx.update_window(window, |_, window, cx| {
            window.dispatch_action(Box::new(super::Zoom), cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn close_window_closes_it(cx: &mut TestAppContext) {
        let window = window(cx);
        cx.update_window(window, |_, window, cx| window.press("cmd-w", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.windows().is_empty()));
    }
}
