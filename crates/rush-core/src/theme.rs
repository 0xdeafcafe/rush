//! rush's colours (from internal/ui/style.go and internal/convo/style.go in
//! the Go version). Screens copy the terminal rush's look; see
//! docs/screenshots.

use crate::text::Color;

/// The palette one ground (dark terminal) uses.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub text: Color,
    pub sub: Color,
    pub dim: Color,
    pub faint: Color,
    pub orange: Color,
    pub yellow: Color,
    pub blue: Color,
    pub green: Color,
    pub red: Color,
    pub sel_bg: Color,
    pub hover_bg: Color,
    pub panel_bg: Color,
    /// The user's messages in a conversation.
    pub user_bg: Color,
    /// dim and faint as the conversation raises them.
    pub convo_dim: Color,
    pub convo_faint: Color,
}

pub const DARK: Palette = Palette {
    text: Color::Rgb(226, 221, 211),
    sub: Color::Rgb(168, 162, 152),
    dim: Color::Rgb(122, 117, 108),
    faint: Color::Rgb(72, 68, 63),
    orange: Color::Rgb(217, 119, 87),
    yellow: Color::Rgb(229, 181, 103),
    blue: Color::Rgb(143, 179, 217),
    green: Color::Rgb(127, 191, 138),
    red: Color::Rgb(224, 104, 92),
    sel_bg: Color::Rgb(44, 40, 36),
    hover_bg: Color::Rgb(33, 31, 29),
    panel_bg: Color::Rgb(30, 28, 26),
    user_bg: Color::Rgb(0x1a, 0x18, 0x16),
    convo_dim: Color::Rgb(138, 132, 122),
    convo_faint: Color::Rgb(94, 89, 82),
};

/// Colourblind mode swaps green and red for these.
pub const COLOURBLIND_GREEN: Color = Color::Rgb(86, 180, 233);
pub const COLOURBLIND_RED: Color = Color::Rgb(230, 159, 0);
