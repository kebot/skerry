//! Skerry's assets: gpui-kit's default icon bundle plus the few Lucide icons it leaves out.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(ExtraIcons, [Trash, FolderPlus, LogOut, Circle]);

/// gpui-kit's default icons, the Files window's Trash and New Folder icons, and Sign Out's
/// LogOut icon, and the launch checklist's Circle.
#[derive(Clone, Copy, Debug, Default)]
pub struct SkerryAssets;

impl AssetSource for SkerryAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // The default bundle answers an unknown path with an error, not `None`.
        match gpui_kit::assets::Assets.load(path) {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            _ => ExtraIcons.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names = gpui_kit::assets::Assets.list(path).unwrap_or_default();
        names.extend(ExtraIcons.list(path)?);
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extra_icons_load() {
        for icon in [
            gpui_kit::assets::IconName::Trash,
            gpui_kit::assets::IconName::FolderPlus,
            gpui_kit::assets::IconName::LogOut,
            gpui_kit::assets::IconName::Circle,
        ] {
            let path = icon.path();
            assert!(
                SkerryAssets.load(&path).unwrap().is_some(),
                "{path} does not load"
            );
        }
        assert!(SkerryAssets.load("icons/folder.svg").unwrap().is_some());
    }
}
