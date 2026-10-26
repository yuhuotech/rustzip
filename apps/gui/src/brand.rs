//! RustZIP's colours (docs/ui-spec.md, section 2) applied on top of the kit's theme.
//!
//! Colours are only ever read through the theme, never written into a view, so the
//! light and dark palettes stay in one place.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, px, rgb};

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

    /// The palette of a theme with the default accent.
    pub fn themed(dark: bool) -> Self {
        Self::for_mode(dark).with_accent(DEFAULT_ACCENT, dark)
    }

    /// The same palette with another accent. Anything unknown gets the default accent.
    pub fn with_accent(mut self, spec: &str, dark: bool) -> Self {
        let Some(pick) = resolve(spec).or_else(|| resolve(DEFAULT_ACCENT)) else {
            return self;
        };
        let (accent, stock) = if dark {
            (pick.dark, pick.stock_dark)
        } else {
            (pick.light, pick.stock_light)
        };
        self.accent = hex(accent);
        match stock {
            // Dark green keeps its hand-picked companions.
            Some((soft, ink)) => {
                self.accent_soft = hex(soft);
                self.accent_ink = hex(ink);
            }
            None => {
                // A tint of the accent on the window colour; legible text on top of it.
                let (a, w): (Rgba, Rgba) = (self.accent.into(), self.win.into());
                let keep = if dark { 0.2 } else { 0.12 };
                let tint = |a: f32, w: f32| a * keep + w * (1. - keep);
                self.accent_soft = Rgba {
                    r: tint(a.r, w.r),
                    g: tint(a.g, w.g),
                    b: tint(a.b, w.b),
                    a: 1.,
                }
                .into();
                let luma = 0.2126 * a.r + 0.7152 * a.g + 0.0722 * a.b;
                self.accent_ink = if luma > 0.62 {
                    hex(0x0b1115)
                } else {
                    hex(0xffffff)
                };
            }
        }
        self
    }
}

/// A selectable accent: its colour in the light and dark themes.
#[derive(Clone, Copy)]
struct Preset {
    id: &'static str,
    light: u32,
    dark: u32,
    /// `(soft, ink)` to use as they are instead of deriving them.
    stock_light: Option<(u32, u32)>,
    stock_dark: Option<(u32, u32)>,
}

/// What a new install and any unreadable setting use.
pub const DEFAULT_ACCENT: &str = "graphite";

const PRESETS: [Preset; 5] = [
    // Near black in light mode and near white in dark mode: the quietest choice.
    Preset {
        id: "graphite",
        light: 0x2b3640,
        dark: 0xe3e9ee,
        stock_light: None,
        stock_dark: None,
    },
    Preset {
        id: "teal",
        light: 0x0d9b82,
        dark: 0x2fd3a7,
        stock_light: Some((0xdff4ee, 0xffffff)),
        stock_dark: Some((0x11342c, 0x04231b)),
    },
    Preset {
        id: "blue",
        light: 0x2457c5,
        dark: 0x6f9bff,
        stock_light: None,
        stock_dark: None,
    },
    Preset {
        id: "violet",
        light: 0x6a45c9,
        dark: 0xa98bff,
        stock_light: None,
        stock_dark: None,
    },
    Preset {
        id: "rose",
        light: 0xc43e66,
        dark: 0xff86a5,
        stock_light: None,
        stock_dark: None,
    },
];

/// A preset by name, or a `#rrggbb` colour used in both themes.
fn resolve(spec: &str) -> Option<Preset> {
    let spec = spec.trim();
    if let Some(p) = PRESETS.iter().find(|p| p.id == spec) {
        return Some(*p);
    }
    let digits = spec.strip_prefix('#').unwrap_or(spec);
    if digits.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(digits, 16).ok()?;
    Some(Preset {
        id: "custom",
        light: v,
        dark: v,
        stock_light: None,
        stock_dark: None,
    })
}

/// `(name, label, colour)` of every preset as it looks in the given theme.
pub fn accents(dark: bool, language: &str) -> Vec<(&'static str, &'static str, Hsla)> {
    PRESETS
        .iter()
        .map(|p| {
            let key = match p.id {
                "graphite" => "accent_graphite",
                "teal" => "accent_teal",
                "blue" => "accent_blue",
                "violet" => "accent_violet",
                "rose" => "accent_rose",
                _ => "accent_custom",
            };
            (
                p.id,
                crate::text::tr(language, key),
                hex(if dark { p.dark } else { p.light }),
            )
        })
        .collect()
}

/// The accent the user picked; the default when nothing was chosen yet.
struct AccentChoice(String);
impl gpui_kit::Global for AccentChoice {}

/// The palette for the theme that is active right now.
pub fn palette(cx: &App) -> Palette {
    let dark = Theme::global(cx).is_dark();
    let base = Palette::for_mode(dark);
    match cx.try_global::<AccentChoice>() {
        Some(c) => base.with_accent(&c.0, dark),
        None => base.with_accent(DEFAULT_ACCENT, dark),
    }
}

/// Remembers the accent without repainting; `choose` or `apply` does that.
pub fn set_accent(spec: &str, cx: &mut App) {
    cx.set_global(AccentChoice(spec.to_string()));
}

/// Paints the active theme with RustZIP's colours.
///
/// Call it after every `Theme::sync_system_appearance`: changing the mode reloads
/// the kit's default colours, which would undo this.
pub fn apply(cx: &mut App) {
    let p = palette(cx);
    // A little lighter or darker for hover and press, in whichever direction stays visible.
    let shade = |c: Hsla, by: f32| Hsla {
        l: (c.l + by).clamp(0., 1.),
        ..c
    };
    Theme::update(cx, |t| {
        t.font_size = px(13.);
        t.button_primary = p.accent;
        t.button_primary_hover = shade(p.accent, if p.win.l < 0.5 { 0.06 } else { -0.05 });
        t.button_primary_active = shade(p.accent, if p.win.l < 0.5 { 0.1 } else { -0.09 });
        t.button_primary_foreground = p.accent_ink;
        t.primary_hover = t.button_primary_hover;
        t.primary_active = t.button_primary_active;
        t.progress_bar = p.accent;
        t.ring = p.accent;
        t.caret = p.accent;
        t.link = p.accent;
        t.link_hover = shade(p.accent, if p.win.l < 0.5 { 0.08 } else { -0.06 });
        t.input = p.line;
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
        t.table = p.win;
        t.table_even = p.win;
        t.table_active = p.accent_soft;
        t.table_hover = p.hover;
        t.table_head = p.win;
        t.table_head_foreground = p.fg3;
        t.colors.list = p.win;
        t.colors.list_even = p.win;
        t.colors.list_head = p.win;
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
            // A fixed light or dark choice must survive the system switching.
            if cx
                .try_global::<Choice>()
                .is_none_or(|c| c.0 == Pick::System)
            {
                Theme::sync_system_appearance(Some(window), cx);
                apply(cx);
            }
        })
        .detach();
}

pub use rz_app::settings::Theme as Pick;

struct Choice(Pick);
impl gpui_kit::Global for Choice {}

/// Applies the theme the user picked in the settings.
pub fn choose(pick: Pick, window: &mut gpui_kit::Window, cx: &mut App) {
    cx.set_global(Choice(pick));
    match pick {
        Pick::System => Theme::sync_system_appearance(Some(window), cx),
        Pick::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Pick::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
    apply(cx);
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

    #[test]
    fn accents_resolve_presets_custom_colours_and_fall_back() {
        let base = Palette::light();
        // Dark green keeps its hand-picked companions.
        assert_eq!(
            base.with_accent("teal", false).accent_soft,
            base.accent_soft
        );
        // Unknown or malformed values get the default, which is graphite.
        let default = base.with_accent(DEFAULT_ACCENT, false).accent;
        assert_eq!(default, hex(0x2b3640));
        assert_eq!(base.with_accent("nonsense", false).accent, default);
        assert_eq!(base.with_accent("#12345", false).accent, default);
        assert_eq!(Palette::themed(false).accent, default);
        // Presets and hex colours are applied.
        assert_eq!(base.with_accent("graphite", false).accent, hex(0x2b3640));
        assert_eq!(base.with_accent("#C43E66", false).accent, hex(0xc43e66));
        // Text on a pale accent is dark, on a deep one white.
        let dark = Palette::dark();
        assert_eq!(dark.with_accent("graphite", true).accent_ink, hex(0x0b1115));
        assert_eq!(
            base.with_accent("graphite", false).accent_ink,
            hex(0xffffff)
        );
        // The tint differs from the window so selections stay visible.
        assert_ne!(base.with_accent("blue", false).accent_soft, base.win);
    }
}
