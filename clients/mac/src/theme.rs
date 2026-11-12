//! The Mac app's palette: Renoa's Paper (light) and Graphite (dark) surfaces,
//! ink as the accent, and the agents' tile pigments.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use gpui_kit::{
    App, Global, Hsla, Rems, Window, WindowAppearance, component::Theme, px, rems, rgb, rgba,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Hsla,
    pub sidebar: Hsla,
    pub selected: Hsla,
    pub hover: Hsla,
    pub text: Hsla,
    pub text_soft: Hsla,
    pub secondary: Hsla,
    pub chat_agent: Hsla,
    pub chat_user: Hsla,
    pub composer: Hsla,
    pub outline: Hsla,
    pub outline_strong: Hsla,
    pub frosted: Hsla,
    pub attention: Hsla,
    pub attention_wash: Hsla,
    pub attention_border: Hsla,
    pub danger: Hsla,
    /// The tiles' colours: clay, cobalt and ochre.
    pub clay: Hsla,
    pub cobalt: Hsla,
    pub ochre: Hsla,
    /// What dims the chats behind a sheet.
    pub scrim: Hsla,
    /// A sheet's or card's own ground.
    pub card: Hsla,
    /// An active segment or lifted surface inside a card.
    pub surface_raised: Hsla,
    /// The ground of a group of rows on a sheet.
    pub group: Hsla,
    /// The line between rows of a group.
    pub divider: Hsla,
    /// Translucent text selection highlight.
    pub selection: Hsla,
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
            background: rgb(0xf5f6f3).into(),
            sidebar: rgb(0xeceee9).into(),
            selected: rgb(0xe1e4dd).into(),
            hover: rgba(0x14161a0a).into(),
            text: rgb(0x14161a).into(),
            text_soft: rgb(0x343940).into(),
            secondary: rgb(0x63686e).into(),
            chat_agent: rgb(0xe8ebe5).into(),
            chat_user: rgb(0xe9ece6).into(),
            composer: rgb(0xffffff).into(),
            outline: rgba(0x14161a1f).into(),
            outline_strong: rgba(0x14161a38).into(),
            frosted: rgba(0xf5f6f3e8).into(),
            attention: rgb(0x9e6814).into(),
            attention_wash: rgba(0xb37b2214).into(),
            attention_border: rgba(0xb37b2238).into(),
            danger: rgb(0xb3321e).into(),
            clay: rgb(0xc4553f).into(),
            cobalt: rgb(0x3b61ac).into(),
            ochre: rgb(0xb37b22).into(),
            scrim: rgba(0x14161a47).into(),
            card: rgb(0xffffff).into(),
            surface_raised: rgb(0xffffff).into(),
            group: rgba(0x14161a07).into(),
            divider: rgb(0xe2e4de).into(),
            selection: rgba(0x3b61ac29).into(),
        }
    }

    fn dark() -> Self {
        Self {
            background: rgb(0x111312).into(),
            sidebar: rgb(0x0c0e0d).into(),
            selected: rgb(0x212523).into(),
            hover: rgba(0xeceee90d).into(),
            text: rgb(0xeceee9).into(),
            text_soft: rgb(0xd0d4ce).into(),
            secondary: rgb(0x8e948d).into(),
            chat_agent: rgb(0x212523).into(),
            chat_user: rgb(0x1d211f).into(),
            composer: rgb(0x181b19).into(),
            outline: rgba(0xeceee91c).into(),
            outline_strong: rgba(0xeceee936).into(),
            frosted: rgba(0x181b19eb).into(),
            attention: rgb(0xe3a94d).into(),
            attention_wash: rgba(0xd9a24b14).into(),
            attention_border: rgba(0xd9a24b33).into(),
            danger: rgb(0xec8379).into(),
            clay: rgb(0xe27b63).into(),
            cobalt: rgb(0x7298de).into(),
            ochre: rgb(0xd9a24b).into(),
            scrim: rgba(0x00000088).into(),
            card: rgb(0x181b19).into(),
            surface_raised: rgb(0x252927).into(),
            group: rgba(0xeceee909).into(),
            divider: rgb(0x242825).into(),
            selection: rgba(0x7298de38).into(),
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

/// The size the app is drawn at, as a browser zooms a page: 1.0 is as
/// designed. Every length the app sets is in [`u`], which scales with it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Zoom(f32);

impl Global for Zoom {}

/// The sizes ⌘= and ⌘- step through, as Safari's.
const ZOOMS: [f32; 9] = [0.75, 0.85, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5];

impl Default for Zoom {
    fn default() -> Self {
        Self(1.0)
    }
}

impl Zoom {
    /// The next size up (`step` 1) or down (`step` -1), held at the ends.
    #[must_use]
    pub fn step(self, step: isize) -> Self {
        let here = ZOOMS
            .iter()
            .position(|zoom| *zoom >= self.0 - f32::EPSILON)
            .unwrap_or(ZOOMS.len() - 1);
        let next = here.saturating_add_signed(step).min(ZOOMS.len() - 1);
        Self(ZOOMS[next])
    }

    fn known(self) -> Self {
        if ZOOMS.contains(&self.0) {
            self
        } else {
            Self::default()
        }
    }
}

/// The app's unit of length: `v` points at the designed size, scaled by the
/// owner's [`Zoom`] through the window's rem size.
#[must_use]
pub fn u(v: f32) -> Rems {
    rems(v / DESIGN_REM)
}

/// The rem size the app is designed at, in points.
const DESIGN_REM: f32 = 16.0;

/// The kit's own text sizes at [`Zoom`] 1.0 (`gpui_kit::component::Theme`).
const KIT_TEXT: f32 = 14.5;
const KIT_MONO_TEXT: f32 = 12.5;

/// What the app keeps of the owner's choices, beside the Keychain's secrets.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    appearance: Appearance,
    #[serde(default)]
    zoom: Option<Zoom>,
}

fn kept_at() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/live.renoa.mac/settings.json"))
}

/// The appearance the owner chose last, or [`Appearance::System`] when none
/// was kept or it does not read.
pub fn kept_appearance() -> Appearance {
    kept_at().map_or_else(Appearance::default, |path| read_kept(&path).appearance)
}

/// The size the owner chose last, or 1.0 when none was kept or it is not
/// one of the sizes the keys step through.
pub fn kept_zoom() -> Zoom {
    kept_at()
        .and_then(|path| read_kept(&path).zoom)
        .map_or_else(Zoom::default, Zoom::known)
}

fn read_kept(path: &Path) -> Kept {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .unwrap_or_default()
}

/// Keeps `appearance` for the next launch, beside the kept size.
pub fn keep_appearance(appearance: Appearance) -> io::Result<()> {
    let path = kept_at().ok_or_else(|| io::Error::other("no home folder"))?;
    let kept = read_kept(&path);
    keep(&path, &Kept { appearance, ..kept })
}

/// Keeps `zoom` for the next launch, beside the kept appearance.
#[cfg_attr(
    test,
    expect(dead_code, reason = "tests leave the owner's kept choices alone")
)]
pub fn keep_zoom(zoom: Zoom) -> io::Result<()> {
    let path = kept_at().ok_or_else(|| io::Error::other("no home folder"))?;
    let kept = read_kept(&path);
    keep(
        &path,
        &Kept {
            zoom: Some(zoom),
            ..kept
        },
    )
}

/// Written beside, then moved over, so a crash leaves the old choices or the
/// new ones.
fn keep(path: &Path, kept: &Kept) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let json = serde_json::to_vec_pretty(kept).map_err(io::Error::other)?;
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

/// Draws the app at `zoom`: the window's rem size, which every [`u`] length
/// follows, and the kit's text sizes.
pub fn zoom(zoom: Zoom, window: &mut Window, cx: &mut App) {
    cx.set_global(zoom);
    sync(window, cx);
}

/// Brings the kit's components in line with the app's look and size, after
/// the owner chose or the system changed.
pub fn sync(window: &mut Window, cx: &mut App) {
    let scale = cx.try_global::<Zoom>().copied().unwrap_or_default().0;
    window.set_rem_size(px(DESIGN_REM * scale));
    let dark = dark(window, cx);
    let palette = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let mode = if dark {
        gpui_kit::component::ThemeMode::Dark
    } else {
        gpui_kit::component::ThemeMode::Light
    };
    if Theme::global(cx).mode != mode {
        Theme::change(mode, Some(window), cx);
    }
    {
        let theme = Theme::global_mut(cx);
        theme.font_size = px(KIT_TEXT * scale);
        theme.mono_font_size = px(KIT_MONO_TEXT * scale);
        theme.colors.background = palette.background;
        theme.colors.foreground = palette.text;
        theme.colors.primary = palette.text;
        theme.colors.secondary = palette.chat_agent;
        theme.colors.muted = palette.card;
        theme.colors.muted_foreground = palette.secondary;
        theme.colors.accent = palette.selected;
        theme.colors.border = palette.divider;
        theme.colors.popover = palette.card;
        theme.colors.link = palette.cobalt;
        theme.colors.selection = palette.selection;
        theme.colors.caret = palette.text;
        theme.colors.scrollbar = palette.background;
    }
    Theme::sync_base(cx);
    window.refresh();
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Appearance, Kept, Zoom, keep, read_kept};

    #[test]
    fn a_kept_appearance_comes_back_and_anything_else_reads_as_the_system() {
        let folder = std::env::temp_dir().join(format!("renoa-mac-kept-{}", uuid::Uuid::new_v4()));
        let path = folder.join("settings.json");
        let appearance = |path| read_kept(path).appearance;
        assert_eq!(appearance(&path), Appearance::System, "nothing kept yet");
        keep(
            &path,
            &Kept {
                appearance: Appearance::Dark,
                zoom: None,
            },
        )
        .expect("kept");
        assert_eq!(appearance(&path), Appearance::Dark);
        keep(
            &path,
            &Kept {
                appearance: Appearance::Light,
                zoom: Some(Zoom(1.25)),
            },
        )
        .expect("kept again");
        assert_eq!(appearance(&path), Appearance::Light);
        assert_eq!(
            read_kept(&path).zoom,
            Some(Zoom(1.25)),
            "the size is kept beside it"
        );
        assert!(
            !path.with_extension("json.new").exists(),
            "the new choice was moved over the old"
        );
        fs::write(&path, b"{\"appearance\":\"sepia\"}").expect("written");
        assert_eq!(
            appearance(&path),
            Appearance::System,
            "a choice it does not know"
        );
        fs::remove_dir_all(folder).expect("cleaned up");
    }

    #[test]
    fn the_keys_step_through_the_sizes_and_stop_at_the_ends() {
        assert_eq!(Zoom::default().step(1), Zoom(1.1));
        assert_eq!(Zoom(1.1).step(-1), Zoom(1.0));
        assert_eq!(Zoom(0.75).step(-1), Zoom(0.75));
        assert_eq!(Zoom(2.5).step(1), Zoom(2.5));
        assert_eq!(
            Zoom(1.3).known(),
            Zoom::default(),
            "a size the keys never reach"
        );
    }
}
