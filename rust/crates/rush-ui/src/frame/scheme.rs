//! The terminal's colours and whether the system went dark or light:
//! asked once as rush starts (OSC 10 and 11) and reported as they change
//! (mode 2031, which Ghostty, kitty and others send), so rush's colours
//! can be made for its ground (ground.rs). After Go's
//! internal/ui/scheme.go (startColours :19, StopSchemeReports :23) and
//! model.go:553 (askColours, applyColors).
//!
//! rush-draw asks for the colours as it takes the terminal, before its
//! input thread reads, and keeps the replies (rush_draw::colour_replies);
//! these read them. Not yet: asking again on focus or on a mode 2031
//! report, so a terminal switched light or dark while rush runs keeps
//! the colours it had (DESIGN.md "Screens", gaps).

use super::ground::Rgb;

/// Asks the terminal for its text and background colours.
pub const ASK_COLOURS: &str = "\x1b]10;?\x1b\\\x1b]11;?\x1b\\";
/// Asks to be told when the system's light or dark changes (mode 2031).
pub const SET_MODE_LIGHT_DARK: &str = "\x1b[?2031h";
/// Turns those reports off again as rush exits, so they don't land in
/// the shell (StopSchemeReports).
pub const RESET_MODE_LIGHT_DARK: &str = "\x1b[?2031l";

/// A colour report: OSC 10 (text) or 11 (background) with its colour,
/// `\x1b]11;rgb:1111/1010/0e0e\x07` or ST-ended.
pub fn parse_colour_report(s: &str) -> Option<(u8, Rgb)> {
    let body = s.strip_prefix("\x1b]")?;
    let body = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))?;
    let (n, spec) = body.split_once(';')?;
    let n: u8 = n.parse().ok()?;
    let rgb = spec.strip_prefix("rgb:")?;
    let mut parts = rgb.split('/');
    let mut ch = || -> Option<u8> {
        let p = parts.next()?;
        // 1 to 4 hex digits, as X11 says; anything else isn't a colour.
        if p.is_empty() || p.len() > 4 || !p.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(p, 16).ok()?;
        // 1 to 4 hex digits, scaled to 8 bits.
        let max = (1u32 << (4 * p.len() as u32)) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    Some((n, Rgb(ch()?, ch()?, ch()?)))
}

/// A mode 2031 report: `\x1b[?997;1n` dark, `\x1b[?997;2n` light.
pub fn parse_scheme_report(s: &str) -> Option<bool> {
    match s {
        "\x1b[?997;1n" => Some(true),
        "\x1b[?997;2n" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reports() {
        assert_eq!(
            parse_colour_report("\x1b]11;rgb:1111/1010/0e0e\x07"),
            Some((11, Rgb(17, 16, 14)))
        );
        assert_eq!(
            parse_colour_report("\x1b]10;rgb:ff/ff/ff\x1b\\"),
            Some((10, Rgb(255, 255, 255)))
        );
        assert_eq!(parse_colour_report("\x1b]11;?\x07"), None);
        // Parts that aren't 1 to 4 hex digits.
        for bad in [
            "\x1b]11;rgb:111111111/0/0\x07",
            "\x1b]11;rgb:11111/0/0\x07",
            "\x1b]11;rgb:/0/0\x07",
            "\x1b]11;rgb:+f/0/0\x07",
        ] {
            assert_eq!(parse_colour_report(bad), None, "{bad:?}");
        }
        assert_eq!(parse_scheme_report("\x1b[?997;1n"), Some(true));
        assert_eq!(parse_scheme_report("\x1b[?997;2n"), Some(false));
        assert_eq!(parse_scheme_report("x"), None);
    }
}
