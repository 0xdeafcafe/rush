//! rush's colours made from the terminal's own: its background and its
//! text. Every colour is written as it looks on rush's dark ground
//! (DARK), and moved onto the terminal's by what kind of colour it is:
//! text (ink), a surface, or an accent. After photon/theme/theme.go
//! (Mix :34, Contrast :54, Ground :62, Terminal :76, Ink :112, Surface
//! :144, Accent :180, Quiet :197), which Go rush's ui/style.go:51
//! (applyColors) calls.

use rush_core::text::Color;

/// A 24-bit colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn color(self) -> Color {
        Color::Rgb(self.0, self.1, self.2)
    }

    pub fn of(c: Color) -> Option<Rgb> {
        match c {
            Color::Rgb(r, g, b) => Some(Rgb(r, g, b)),
            _ => None,
        }
    }

    /// Nearer black than white, to the eye.
    pub fn dark(self) -> bool {
        contrast(self, Rgb(255, 255, 255)) > contrast(self, Rgb(0, 0, 0))
    }
}

fn clamp(v: f64) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

/// t of the way from a to b; past 0 or 1 carries on beyond them.
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let l = |x: u8, y: u8| clamp(x as f64 + (y as f64 - x as f64) * t);
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

fn luminance(c: Rgb) -> f64 {
    let ch = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.0) + 0.7152 * ch(c.1) + 0.0722 * ch(c.2)
}

/// The WCAG contrast ratio of a and b, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A terminal's background and text, what every colour is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ground {
    pub bg: Rgb,
    pub fg: Rgb,
}

/// rush's own ground, the one its colours are written for, and what's
/// used when the terminal doesn't say.
pub const DARK: Ground = Ground {
    bg: Rgb(17, 16, 14),
    fg: Rgb(226, 221, 211),
};
/// rush's light ground, for a light terminal that doesn't say its text.
pub const LIGHT: Ground = Ground {
    bg: Rgb(250, 249, 245),
    fg: Rgb(40, 37, 32),
};

/// The ground rush's colours are made for (Go's style.go:43 painted):
/// DARK until the terminal says otherwise.
static PAINTED: std::sync::RwLock<Ground> = std::sync::RwLock::new(DARK);

/// The ground rush's colours are made for now (painted).
pub fn painted() -> Ground {
    *PAINTED.read().unwrap_or_else(|e| e.into_inner())
}

/// Makes g the ground rush's colours are for (applyColors' painted = g):
/// the terminal's answer to OSC 11, once it's asked.
pub fn set_painted(g: Ground) {
    *PAINTED.write().unwrap_or_else(|e| e.into_inner()) = g;
}

/// The ground Settings › Theme picks (model.go:569 applyColors): rush's
/// dark or light, or for "match terminal" the terminal's own background
/// and text, as it answered OSC 10 and 11 when rush took it; rush's dark
/// when it didn't say.
pub fn of_theme(theme: &str) -> Ground {
    match theme {
        "light" => LIGHT,
        "dark" => DARK,
        _ => answered(rush_draw::colour_replies()),
    }
}

/// The terminal's ground from its OSC 10 and 11 replies (theme.Terminal).
pub fn answered(replies: Vec<String>) -> Ground {
    let (mut fg, mut bg) = (None, None);
    for r in replies {
        match super::scheme::parse_colour_report(&r) {
            Some((10, c)) => fg = Some(c),
            Some((11, c)) => bg = Some(c),
            _ => {}
        }
    }
    terminal(bg, fg)
}

/// c, a colour written for DARK, as it's drawn on g (applyColors): a
/// background is a surface, a grey is text (ink), anything with a hue an
/// accent. colour_blind makes good and bad sky blue and amber, not green
/// and red.
pub fn recoloured(g: Ground, colour_blind: bool, c: Color, bg: bool) -> Color {
    use rush_core::theme;
    let Some(mut rgb) = Rgb::of(c) else {
        return c;
    };
    // Text and a button filled in the colour alike (btnFill,
    // style.go:68-76).
    if colour_blind {
        if c == theme::DARK.green {
            rgb = Rgb::of(theme::COLOURBLIND_GREEN).unwrap_or(rgb);
        } else if c == theme::DARK.red {
            rgb = Rgb::of(theme::COLOURBLIND_RED).unwrap_or(rgb);
        }
    }
    if g == DARK {
        return rgb.color();
    }
    let spread = rgb.0.max(rgb.1).max(rgb.2) - rgb.0.min(rgb.1).min(rgb.2);
    if bg {
        g.surface(rgb).color()
    } else if spread < 30 {
        g.ink(rgb).color()
    } else {
        g.accent(rgb).color()
    }
}

/// The ground and colour-blind choice the drawing is recoloured for now.
static APPLIED: std::sync::Mutex<(Ground, bool)> = std::sync::Mutex::new((DARK, false));

/// Makes every colour drawn from now on for ground g, colour-blind or
/// not (applyColors): nothing is changed for rush's own dark in standard
/// colours. Each colour is worked out once and kept.
pub fn apply(g: Ground, colour_blind: bool) {
    let mut was = APPLIED.lock().unwrap_or_else(|e| e.into_inner());
    if *was == (g, colour_blind) {
        return;
    }
    *was = (g, colour_blind);
    if g == DARK && !colour_blind {
        rush_draw::set_recolour(None);
        return;
    }
    let made: std::sync::Mutex<std::collections::HashMap<(Color, bool), Color>> =
        std::sync::Mutex::new(std::collections::HashMap::new());
    rush_draw::set_recolour(Some(Box::new(move |c, bg| {
        let mut m = made.lock().unwrap_or_else(|e| e.into_inner());
        *m.entry((c, bg))
            .or_insert_with(|| recoloured(g, colour_blind, c, bg))
    })));
}

/// The contrast text needs to be read with ease: WCAG's AA.
const READABLE: f64 = 4.5;
/// How much stronger, in log contrast, grey text is drawn on a light
/// ground than its contrast on DARK says.
const LIGHT_BOOST: f64 = 1.15;

/// The ground for a terminal whose background is bg and text fg, either
/// of which it may not have said. Text too close to the background to
/// read is replaced by rush's own for that kind of ground (Terminal).
pub fn terminal(bg: Option<Rgb>, fg: Option<Rgb>) -> Ground {
    let Some(bg) = bg else { return DARK };
    let r = if bg.dark() { DARK } else { LIGHT };
    let mut g = Ground { bg, fg: r.fg };
    if let Some(fg) = fg
        && contrast(bg, fg) >= READABLE
    {
        g.fg = fg;
    }
    g
}

// How far c lies from a toward b: its projection on the line between.
fn towards(a: Rgb, b: Rgb, c: Rgb) -> f64 {
    let d = |x: u8, y: u8| y as f64 - x as f64;
    let (dr, dg, db) = (d(a.0, b.0), d(a.1, b.1), d(a.2, b.2));
    let (cr, cg, cb) = (d(a.0, c.0), d(a.1, c.1), d(a.2, c.2));
    let n = dr * dr + dg * dg + db * db;
    if n == 0.0 {
        return 0.0;
    }
    (cr * dr + cg * dg + cb * db) / n
}

impl Ground {
    /// Text or a rule, c on DARK: a shade between this ground's
    /// background and text that stands out as c does on DARK, scaled to
    /// how much the terminal's own text does (Ink).
    pub fn ink(self, c: Rgb) -> Rgb {
        if self == DARK {
            return c;
        }
        let t = towards(DARK.bg, DARK.fg, c);
        if t > 1.0 {
            return mix(self.bg, self.fg, t);
        }
        let (was, full) = (contrast(c, DARK.bg), contrast(self.fg, self.bg));
        let mut want = (was.ln() * full.ln() / contrast(DARK.fg, DARK.bg).ln()).exp();
        if !self.bg.dark() {
            want = want.powf(LIGHT_BOOST).min(full);
        }
        if was >= READABLE {
            want = want.max(READABLE.min(full));
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..20 {
            let t = (lo + hi) / 2.0;
            if contrast(mix(self.bg, self.fg, t), self.bg) < want {
                lo = t;
            } else {
                hi = t;
            }
        }
        mix(self.bg, self.fg, hi)
    }

    /// A ground for a row or panel, c on DARK: as far from the background
    /// as c is on DARK's, carrying whatever hue c adds (Surface).
    pub fn surface(self, c: Rgb) -> Rgb {
        if self == DARK {
            return c;
        }
        let d = [
            c.0 as f64 - DARK.bg.0 as f64,
            c.1 as f64 - DARK.bg.1 as f64,
            c.2 as f64 - DARK.bg.2 as f64,
        ];
        let lo = d[0].min(d[1]).min(d[2]);
        let hi = d[0].max(d[1]).max(d[2]);
        let span = (DARK.fg.0 as f64 + DARK.fg.1 as f64 + DARK.fg.2 as f64
            - DARK.bg.0 as f64
            - DARK.bg.1 as f64
            - DARK.bg.2 as f64)
            / 3.0;
        let base = mix(self.bg, self.fg, lo / span);
        let dark = self.bg.dark();
        let hue = |i: usize, v: u8| {
            if dark {
                clamp(v as f64 + d[i] - lo)
            } else {
                clamp(v as f64 - (hi - d[i]))
            }
        };
        Rgb(hue(0, base.0), hue(1, base.1), hue(2, base.2))
    }

    /// A colour that says something, c on DARK: the same colour pushed
    /// toward the text until it stands out from this ground as it does
    /// from DARK's, or is readable (Accent).
    pub fn accent(self, c: Rgb) -> Rgb {
        if self == DARK {
            return c;
        }
        let want = READABLE.min(contrast(c, DARK.bg));
        let end = if self.bg.dark() {
            Rgb(255, 255, 255)
        } else {
            Rgb(0, 0, 0)
        };
        let mut out = c;
        let mut t = 0.05;
        while contrast(out, self.bg) < want && t <= 1.0 {
            out = mix(c, end, t);
            t += 0.05;
        }
        out
    }

    /// An accent held back toward the background, c on DARK (Quiet).
    pub fn quiet(self, c: Rgb, accent: Rgb) -> Rgb {
        if self == DARK {
            return c;
        }
        mix(self.bg, self.accent(accent), towards(DARK.bg, accent, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_as_written() {
        let c = Rgb(122, 117, 108);
        assert_eq!(DARK.ink(c), c);
        assert_eq!(DARK.surface(c), c);
        assert_eq!(DARK.accent(c), c);
    }

    #[test]
    fn themes_recolour() {
        use rush_core::theme;
        // Dark, standard: every colour as written.
        let text = theme::DARK.text;
        assert_eq!(recoloured(DARK, false, text, false), text);
        assert_eq!(
            recoloured(DARK, false, Color::Default, true),
            Color::Default
        );
        // Light: text dark on the light ground, the row picked a light
        // surface, orange still orange and readable.
        let t = Rgb::of(recoloured(LIGHT, false, text, false)).unwrap();
        assert!(!t.dark() || contrast(t, LIGHT.bg) >= 4.5, "{t:?}");
        assert!(t.0 < 100, "dark text: {t:?}");
        let sel = Rgb::of(recoloured(LIGHT, false, theme::DARK.sel_bg, true)).unwrap();
        assert!(!sel.dark(), "{sel:?}");
        let o = Rgb::of(recoloured(LIGHT, false, theme::DARK.orange, false)).unwrap();
        assert!(o.0 > o.2, "{o:?}");
        assert!(contrast(o, LIGHT.bg) >= 3.0);
        // Colour-blind: green and red become sky blue and amber.
        assert_eq!(
            recoloured(DARK, true, theme::DARK.green, false),
            theme::COLOURBLIND_GREEN
        );
        assert_eq!(
            recoloured(DARK, true, theme::DARK.red, false),
            theme::COLOURBLIND_RED
        );
        // A button filled in green or red too.
        assert_eq!(
            recoloured(DARK, true, theme::DARK.green, true),
            theme::COLOURBLIND_GREEN
        );
        assert_eq!(
            recoloured(DARK, true, theme::DARK.red, true),
            theme::COLOURBLIND_RED
        );
        assert_eq!(of_theme("light"), LIGHT);
        assert_eq!(of_theme("dark"), DARK);
        // Matching the terminal: its own ground, rush's dark when it
        // didn't say.
        assert_eq!(answered(Vec::new()), DARK);
        let g = answered(vec![
            "\x1b]11;rgb:ffff/ffff/ffff\x1b\\".into(),
            "\x1b]10;rgb:0000/0000/0000\x07".into(),
        ]);
        assert_eq!((g.bg, g.fg), (Rgb(255, 255, 255), Rgb(0, 0, 0)));
    }

    #[test]
    fn light_inverts() {
        let g = terminal(Some(Rgb(255, 255, 255)), Some(Rgb(0, 0, 0)));
        assert!(!g.bg.dark());
        // Text stays readable; a panel stays near the background.
        let text = g.ink(Rgb(226, 221, 211));
        assert!(contrast(text, g.bg) >= 4.5, "{text:?}");
        let panel = g.surface(Rgb(30, 28, 26));
        assert!(contrast(panel, g.bg) < 1.3, "{panel:?}");
        assert!(contrast(g.accent(Rgb(217, 119, 87)), g.bg) >= 3.0);
        // Unreadable text is replaced.
        let g = terminal(Some(Rgb(250, 250, 250)), Some(Rgb(240, 240, 240)));
        assert_eq!(g.fg, LIGHT.fg);
        assert_eq!(terminal(None, None), DARK);
    }
}
