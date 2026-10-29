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
