//! The Mac app's palette: Codync desktop's black-and-white theme, in light
//! and dark (`licenses/README.md`). Colour comes only from what needs the
//! owner (amber for attention, red for errors) and from the agents' tiles.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use gpui_kit::{App, Global, Hsla, Window, WindowAppearance, component::Theme, rgb, rgba};
use serde::{Deserialize, Serialize};

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
    /// What dims the chats behind a sheet.
    pub scrim: Hsla,
    /// A sheet's own ground.
    pub card: Hsla,
    /// The ground of a group of rows on a sheet.
    pub group: Hsla,
    /// The line between rows of a group.
    pub divider: Hsla,
}

impl Palette {
    /// The palette the owner chose, or with [`Appearance::System`] the
    /// window's, which follows the system's.
    pub fn of(window: &Window, cx: &App) -> Self {
        if dark(window, cx) {
            Self::dark()
        } else {
            Self::light()
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
            scrim: rgba(0x00000066).into(),
            card: rgb(0xffffff).into(),
            group: rgba(0x1414140a).into(),
            divider: rgb(0xe9e9e9).into(),
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
            scrim: rgba(0x00000099).into(),
            card: rgb(0x161616).into(),
            group: rgba(0xfafafa0d).into(),
            divider: rgb(0x262626).into(),
        }
    }
}

/// How the owner wants the app to look.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// As the system looks.
    #[default]
    System,
    Light,
    Dark,
}

impl Global for Appearance {}

/// Whether the app looks dark: as the owner chose, or as the window is.
fn dark(window: &Window, cx: &App) -> bool {
    match cx.try_global::<Appearance>().copied().unwrap_or_default() {
        Appearance::Light => false,
        Appearance::Dark => true,
        Appearance::System => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

/// What the app keeps of the owner's choices, beside the Keychain's secrets.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    appearance: Appearance,
}

fn kept_at() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/live.renoa.mac/settings.json"))
}

/// The appearance the owner chose last, or [`Appearance::System`] when none
/// was kept or it does not read.
pub fn kept_appearance() -> Appearance {
    kept_at().map_or_else(Appearance::default, |path| read_kept(&path))
}

fn read_kept(path: &Path) -> Appearance {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .map(|kept| kept.appearance)
        .unwrap_or_default()
}

/// Keeps `appearance` for the next launch: written beside, then moved over,
/// so a crash leaves the old choice or the new one.
pub fn keep_appearance(appearance: Appearance) -> io::Result<()> {
    let path = kept_at().ok_or_else(|| io::Error::other("no home folder"))?;
    keep(&path, appearance)
}

fn keep(path: &Path, appearance: Appearance) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let json = serde_json::to_vec_pretty(&Kept { appearance }).map_err(io::Error::other)?;
    let beside = path.with_extension("json.new");
    fs::write(&beside, json)?;
    fs::rename(beside, path)
}

/// Makes the app look as `appearance` says: its own palette and the kit's
/// components (Markdown, inputs, scrollbars) alike.
pub fn apply(appearance: Appearance, window: &mut Window, cx: &mut App) {
    cx.set_global(appearance);
    sync(window, cx);
}

/// Brings the kit's components in line with the app's look, after the owner
/// chose or the system changed.
pub fn sync(window: &mut Window, cx: &mut App) {
    let dark = dark(window, cx);
    let mode = if dark {
        gpui_kit::component::ThemeMode::Dark
    } else {
        gpui_kit::component::ThemeMode::Light
    };
    if Theme::global(cx).mode != mode {
        Theme::change(mode, Some(window), cx);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Appearance, keep, read_kept};

    #[test]
    fn a_kept_appearance_comes_back_and_anything_else_reads_as_the_system() {
        let folder = std::env::temp_dir().join(format!("renoa-mac-kept-{}", uuid::Uuid::new_v4()));
        let path = folder.join("settings.json");
        assert_eq!(read_kept(&path), Appearance::System, "nothing kept yet");
        keep(&path, Appearance::Dark).expect("kept");
        assert_eq!(read_kept(&path), Appearance::Dark);
        keep(&path, Appearance::Light).expect("kept again");
        assert_eq!(read_kept(&path), Appearance::Light);
        assert!(
            !path.with_extension("json.new").exists(),
            "the new choice was moved over the old"
        );
        fs::write(&path, b"{\"appearance\":\"sepia\"}").expect("written");
        assert_eq!(
            read_kept(&path),
            Appearance::System,
            "a choice it does not know"
        );
        fs::remove_dir_all(folder).expect("cleaned up");
    }
}
