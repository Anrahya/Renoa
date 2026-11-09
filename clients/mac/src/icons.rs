//! The icons the app embeds: the kit's default set, which its components
//! draw with, and the few others the app shows.

use std::borrow::Cow;

use gpui_kit::{
    AssetSource, Result, SharedString,
    assets::{Assets, icon_assets},
};

icon_assets!(Extra, [House]);

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match Extra.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut listed = Extra.list(path)?;
        listed.extend(Assets.list(path)?);
        Ok(listed)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AssetSource as _, assets::IconName};

    use super::Icons;

    /// Every icon the app draws, so a missing one fails here rather than
    /// showing blank.
    #[test]
    fn each_icon_the_app_shows_is_embedded() {
        for icon in [
            IconName::House,
            IconName::Calendar,
            IconName::BookOpen,
            IconName::LayoutDashboard,
            IconName::Settings,
            IconName::Search,
            IconName::Close,
            IconName::Sun,
            IconName::Moon,
        ] {
            let path = icon.path();
            assert!(
                Icons.load(&path).ok().flatten().is_some(),
                "{path} is embedded"
            );
        }
    }
}
