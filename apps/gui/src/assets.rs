//! The icons the window draws: the kit's default set plus a few more from the same
//! Lucide family (ISC licence, see `docs/decisions.md`).

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

icon_assets!(
    Extra,
    [
        Clock,
        ArrowLeftRight,
        Wrench,
        KeyRound,
        ShieldCheck,
        Trash,
        Lock,
        FileArchive,
        Image,
        House,
        List,
        ArrowDownToLine,
        ListChecks,
        LayoutGrid,
    ]
);

/// Every icon the application can draw.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match Extra.load(path)? {
            Some(found) => Ok(Some(found)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = Assets.list(path)?;
        all.extend(Extra.list(path)?);
        Ok(all)
    }
}
