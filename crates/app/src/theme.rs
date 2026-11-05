//! Colour tokens from the design mock (`design/style.css`), light and dark,
//! plus Connection and Tab Group colours. Pure data: no GPUI types except
//! `Hsla` conversions at the edge.

use gpui::{rgb, rgba, Hsla};
use rb_store::GroupColor;

/// A colour as `0xRRGGBBAA`.
pub type Rgba32 = u32;

pub fn hsla(c: Rgba32) -> Hsla {
    rgba(c).into()
}

/// Surface and text tokens for one appearance.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Window / main area background (`--win-solid`).
    pub win: Rgba32,
    /// Sidebar background (the glass colour over the window).
    pub side: Rgba32,
    /// Raised chips inside the Sidebar (`--side-2` / `--field`).
    pub side2: Rgba32,
    pub ink: Rgba32,
    pub ink2: Rgba32,
    pub ink3: Rgba32,
    pub line: Rgba32,
    pub hover: Rgba32,
    /// Active Tab background (`--sel`).
    pub sel: Rgba32,
    pub field: Rgba32,
    pub page: Rgba32,
    pub ok: Rgba32,
    pub warn: Rgba32,
    pub bad: Rgba32,
    pub idle: Rgba32,
    pub accent: Rgba32,
}

pub const LIGHT: Palette = Palette {
    dark: false,
    win: 0xf6f5f9ff,
    side: 0xe9e7f0ff,
    side2: 0xdcd9e7ff,
    ink: 0x1d1b26ff,
    ink2: 0x5d5a6cff,
    ink3: 0x8a869aff,
    line: 0x1e143c1f,
    hover: 0x1e143c12,
    sel: 0xffffffd9,
    field: 0x0000000e,
    page: 0xffffffff,
    ok: 0x22a559ff,
    warn: 0xe0a000ff,
    bad: 0xd93a3aff,
    idle: 0x9a96aaff,
    accent: 0x0a7affff,
};

pub const DARK: Palette = Palette {
    dark: true,
    win: 0x1e1d26ff,
    side: 0x1a1921ff,
    side2: 0x201f29ff,
    ink: 0xecebf3ff,
    ink2: 0xa9a6baff,
    ink3: 0x74718aff,
    line: 0xffffff1a,
    hover: 0xffffff12,
    sel: 0xffffff1f,
    field: 0xffffff14,
    page: 0x12161dff,
    ok: 0x22a559ff,
    warn: 0xe0a000ff,
    bad: 0xd93a3aff,
    idle: 0x9a96aaff,
    accent: 0x0a84ffff,
};

pub fn palette(dark: bool) -> Palette {
    if dark {
        DARK
    } else {
        LIGHT
    }
}

/// Dark for the dark appearances, light otherwise.
pub fn is_dark(a: gpui::WindowAppearance) -> bool {
    matches!(
        a,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

/// The shape of a Connection's dot: the non-colour cue (mock: ● ◆ ▲ ■).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Diamond,
    Triangle,
    Square,
}

impl Shape {
    pub const ALL: [Shape; 4] = [
        Shape::Circle,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Square,
    ];

    pub fn glyph(self) -> char {
        match self {
            Shape::Circle => '●',
            Shape::Diamond => '◆',
            Shape::Triangle => '▲',
            Shape::Square => '■',
        }
    }
}

/// Automatic Connection colours (light, dark), in assignment order: the
/// mock's work, personal, scraper and research.
pub const CONNECTION_PALETTE: [(Rgba32, Rgba32); 4] = [
    (0x2f6fedff, 0x5b93ffff),
    (0xd6409fff, 0xee63b8ff),
    (0xf0a030ff, 0xf5b04aff),
    (0x14a3a8ff, 0x2fc4c9ff),
];

/// Parse `#rrggbb` (as validated by rb-store) to `0xRRGGBBFF`.
pub fn parse_hex(s: &str) -> Option<Rgba32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    u32::from_str_radix(h, 16).ok().map(|v| (v << 8) | 0xff)
}

/// Tab Group colour (light, dark) from the reserved palette. An unknown name
/// (an old file's free-form colour) falls back to a parsed `#rrggbb`, then to
/// violet.
pub fn group_color(name: Option<&str>, dark: bool) -> Rgba32 {
    let pick = |l: Rgba32, d: Rgba32| if dark { d } else { l };
    match name.and_then(GroupColor::from_name) {
        Some(GroupColor::Violet) => pick(0x7c5cf0ff, 0x9d85ffff),
        Some(GroupColor::Olive) => pick(0x7a8a1fff, 0xb3c443ff),
        Some(GroupColor::Stone) => pick(0x8a7560ff, 0xbda58cff),
        None => name
            .and_then(parse_hex)
            .unwrap_or_else(|| pick(0x7c5cf0ff, 0x9d85ffff)),
    }
}

/// Black or white text for a filled badge of colour `c`.
pub fn on_color(c: Rgba32) -> Rgba32 {
    let (r, g, b) = (
        ((c >> 24) & 0xff) as f32,
        ((c >> 16) & 0xff) as f32,
        ((c >> 8) & 0xff) as f32,
    );
    if 0.299 * r + 0.587 * g + 0.114 * b > 150.0 {
        0x1a1200ff
    } else {
        0xffffffff
    }
}

/// `rgb(0xRRGGBB)` to `Hsla` (ignores alpha).
pub fn hsla_rgb(c: u32) -> Hsla {
    rgb(c).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#3b82f6"), Some(0x3b82f6ff));
        assert_eq!(parse_hex("3b82f6"), None);
        assert_eq!(parse_hex("#3b82f"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
    }

    #[test]
    fn group_colours_follow_palette_with_fallbacks() {
        assert_eq!(group_color(Some("violet"), false), 0x7c5cf0ff);
        assert_eq!(group_color(Some("violet"), true), 0x9d85ffff);
        assert_eq!(group_color(Some("olive"), false), 0x7a8a1fff);
        assert_eq!(group_color(Some("#ff0000"), false), 0xff0000ff);
        assert_eq!(group_color(Some("blue"), true), 0x9d85ffff);
        assert_eq!(group_color(None, false), 0x7c5cf0ff);
    }

    #[test]
    fn badge_text_contrasts() {
        assert_eq!(on_color(0xf0a030ff), 0x1a1200ff);
        assert_eq!(on_color(0x2f6fedff), 0xffffffff);
    }

    #[test]
    fn four_distinct_shapes() {
        let g: Vec<char> = Shape::ALL.iter().map(|s| s.glyph()).collect();
        assert_eq!(g, ['●', '◆', '▲', '■']);
    }
}
