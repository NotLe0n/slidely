use std::borrow::Cow;

use gpui_kit::assets::icon_assets;
use gpui_kit::*;

// Icons used by the app that aren't among gpui-kit's default component icons.
// `IconName` lists every icon, but `assets::Assets` only embeds the defaults.
icon_assets!(
    ExtraIcons,
    [
        Captions,
        CircleStop,
        Fullscreen,
        Gauge,
        Presentation,
        RotateCw,
        SquarePlay,
        Volume,
        Volume1,
        Volume2,
        VolumeOff,
        VolumeX,
        PanelLeft,
        PanelRight,
        ZoomIn
    ]
);

/// gpui-kit's default assets plus the extra icons.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
