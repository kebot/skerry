//! Skerry's assets: gpui-kit's default icon bundle plus the few Lucide icons it leaves out.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Trash, FolderPlus, LogOut, Circle, Hourglass, Link, FileUp, FolderUp, Ban, Info, Printer,
        Download, FileX, Crosshair
    ]
);

/// gpui-kit's default icons, the Files window's Trash and New Folder icons, and Sign Out's
/// LogOut icon, the launch checklist’s Circle, the Held panel’s Hourglass, and the open
/// chooser’s Link, FileUp, FolderUp, Ban and Info (`ARCH-028`), and printing’s Printer,
/// Download and FileX (`ARCH-033`).
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
            gpui_kit::assets::IconName::Hourglass,
            gpui_kit::assets::IconName::Link,
            gpui_kit::assets::IconName::FileUp,
            gpui_kit::assets::IconName::FolderUp,
            gpui_kit::assets::IconName::Ban,
            gpui_kit::assets::IconName::Info,
            gpui_kit::assets::IconName::Crosshair,
            gpui_kit::assets::IconName::Printer,
            gpui_kit::assets::IconName::Download,
            gpui_kit::assets::IconName::FileX,
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
