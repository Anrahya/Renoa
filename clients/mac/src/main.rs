//! Renoa for the Mac: a thin chat with the owner's agents over RCP.

mod app;
mod chat;
mod chats;
mod face;
mod home;
mod icons;
mod key;
mod link;
mod menus;
mod sheets;
mod stamp;
#[cfg(test)]
mod test_events;
mod theme;
mod vault;

use gpui_kit::{
    AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};

fn main() {
    gpui_kit::application().with_assets(icons::Icons).run(|cx| {
        gpui_kit::init(cx);
        menus::install(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Renoa".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(16.))),
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1000.), px(700.)),
                cx,
            ))),
            ..WindowOptions::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| app::Renoa::new(window, cx))
        })
        .expect("open the window");
        // The app is its one window.
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
