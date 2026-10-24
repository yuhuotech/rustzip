//! RustZIP's colours (docs/ui-spec.md, section 2) applied on top of the kit's theme.
//!
//! Colours are only ever read through the theme, never written into a view, so the
//! light and dark palettes stay in one place.

use gpui_kit::component::Theme;
use gpui_kit::{App, Hsla, rgb};

/// The semantic tokens of the design: light on the left, dark on the right.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    pub win: Hsla,
    pub panel: Hsla,
    pub line: Hsla,
    pub fg: Hsla,
    pub fg2: Hsla,
    pub fg3: Hsla,
    pub accent: Hsla,
    pub accent_ink: Hsla,
    pub accent_soft: Hsla,
    pub hover: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

impl Palette {
    pub fn light() -> Self {
        Self {
            bg: hex(0xe9edf1),
            win: hex(0xffffff),
            panel: hex(0xf5f7f9),
            line: hex(0xe2e7ec),
            fg: hex(0x121c26),
            fg2: hex(0x56677a),
            fg3: hex(0x8896a4),
            accent: hex(0x0d9b82),
            accent_ink: hex(0xffffff),
            accent_soft: hex(0xdff4ee),
            hover: hex(0xeef2f5),
            warn: hex(0xc98200),
            danger: hex(0xd23f3f),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: hex(0x080c10),
            win: hex(0x11171d),
            panel: hex(0x0d1318),
            line: hex(0x212b34),
            fg: hex(0xe7eef4),
            fg2: hex(0x98a8b6),
            fg3: hex(0x64737f),
            accent: hex(0x2fd3a7),
            accent_ink: hex(0x04231b),
            accent_soft: hex(0x11342c),
            hover: hex(0x18212a),
            warn: hex(0xe5a62a),
            danger: hex(0xef6b6b),
        }
    }

    pub fn for_mode(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }
}

/// The palette for the theme that is active right now.
pub fn palette(cx: &App) -> Palette {
    Palette::for_mode(Theme::global(cx).is_dark())
}

/// Paints the active theme with RustZIP's colours.
///
/// Call it after every `Theme::sync_system_appearance`: changing the mode reloads
/// the kit's default colours, which would undo this.
pub fn apply(cx: &mut App) {
    let p = palette(cx);
    Theme::update(cx, |t| {
        t.background = p.win;
        t.foreground = p.fg;
        t.border = p.line;
        t.muted = p.panel;
        t.muted_foreground = p.fg2;
        t.primary = p.accent;
        t.primary_foreground = p.accent_ink;
        t.accent = p.hover;
        t.accent_foreground = p.fg;
        t.secondary = p.panel;
        t.list_active = p.accent_soft;
        t.list_hover = p.hover;
        t.table_active = p.accent_soft;
        t.table_hover = p.hover;
        t.table_head = p.win;
        t.table_row_border = p.line;
        t.sidebar = p.panel;
        t.title_bar = p.panel;
        t.danger = p.danger;
        t.warning = p.warn;
        t.success = p.accent;
    });
}

/// Follows the system light/dark setting and keeps following it. The kit does
/// not do this by itself.
pub fn follow_system(window: &mut gpui_kit::Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    apply(cx);
    window
        .observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
            apply(cx);
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_palettes_differ_and_match_the_spec() {
        let (l, d) = (Palette::light(), Palette::dark());
        assert_ne!(l.bg, d.bg);
        assert_ne!(l.accent, d.accent);
        // Spot-check against docs/ui-spec.md: accent #0d9b82 / #2fd3a7.
        assert_eq!(l.accent, hex(0x0d9b82));
        assert_eq!(d.accent, hex(0x2fd3a7));
        assert_eq!(Palette::for_mode(true).win, d.win);
        assert_eq!(Palette::for_mode(false).win, l.win);
    }
}
