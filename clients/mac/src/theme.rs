//! The Mac app's palette: Codync desktop's black-and-white theme, in light
//! and dark (`licenses/README.md`). Colour comes only from what needs the
//! owner (amber for attention, red for errors) and from the agents' tiles.

use gpui_kit::{Hsla, Window, WindowAppearance, rgb, rgba};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Hsla,
    pub sidebar: Hsla,
    pub selected: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    pub chat_agent: Hsla,
    pub chat_user: Hsla,
    pub composer: Hsla,
    pub outline: Hsla,
    pub frosted: Hsla,
    pub attention: Hsla,
    pub danger: Hsla,
    /// The tiles' colours: the landing page's clay, cobalt and ochre.
    pub clay: Hsla,
    pub cobalt: Hsla,
    pub ochre: Hsla,
}

impl Palette {
    /// The palette for the window's appearance, which follows the system's.
    pub fn of(window: &Window) -> Self {
        match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    fn light() -> Self {
        Self {
            background: rgb(0xffffff).into(),
            sidebar: rgb(0xf4f4f4).into(),
            selected: rgb(0xececec).into(),
            text: rgb(0x141414).into(),
            secondary: rgb(0x6b6b6b).into(),
            chat_agent: rgb(0xeeeeee).into(),
            chat_user: rgb(0xe2e2e2).into(),
            composer: rgb(0xffffff).into(),
            outline: rgba(0x1414142e).into(),
            frosted: rgba(0xf4f4f4d1).into(),
            attention: rgb(0x936000).into(),
            danger: rgb(0xc23a2b).into(),
            clay: rgb(0xc4553f).into(),
            cobalt: rgb(0x3b61ac).into(),
            ochre: rgb(0xb37b22).into(),
        }
    }

    fn dark() -> Self {
        Self {
            background: rgb(0x070707).into(),
            sidebar: rgb(0x070707).into(),
            selected: rgb(0x292929).into(),
            text: rgb(0xfafafa).into(),
            secondary: rgb(0xa0a0a0).into(),
            chat_agent: rgb(0x262626).into(),
            chat_user: rgb(0x363636).into(),
            composer: rgb(0x2f2f2f).into(),
            outline: rgba(0xfafafa2e).into(),
            frosted: rgba(0x232323d1).into(),
            attention: rgb(0xecaf52).into(),
            danger: rgb(0xf0a7a7).into(),
            clay: rgb(0xe27b63).into(),
            cobalt: rgb(0x7298de).into(),
            ochre: rgb(0xd9a24b).into(),
        }
    }
}
