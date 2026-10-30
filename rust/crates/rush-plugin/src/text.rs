//! Making a plugin's words safe to draw on rush's screen.
//! Ported from internal/plugin/ui.go:369-394 (printable, clip) and
//! sidebar.go:143-204 (cleanLabel, Clean, escLen).

/// printable drops what a terminal would act on rather than show: escapes,
/// control characters, and the bidi overrides that make text read other
/// than it is. A tab is a space.
pub fn printable(s: &str) -> String {
    s.chars()
        .filter_map(|r| {
            let u = r as u32;
            match u {
                0x09 => Some(' '),
                0..=0x1f | 0x7f | 0x80..=0x9f => None,
                0x202a..=0x202e | 0x2066..=0x2069 | 0x200e | 0x200f => None,
                0xfffd => None,
                _ => Some(r),
            }
        })
        .collect()
}

/// clip keeps s to n characters, the last of them … when it's cut.
pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Clean makes s one line of plain text: escape sequences go, and so do
/// control and direction-changing characters, with line breaks and tabs
/// turned into spaces.
pub fn clean(s: &str) -> String {
    let mut b = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let r = s[i..].chars().next().unwrap_or('\u{fffd}');
        if r == '\x1b' {
            i += esc_len(&s.as_bytes()[i..]);
            continue;
        }
        i += r.len_utf8();
        match r {
            '\n' | '\t' | '\r' => b.push(' '),
            r if r.is_control() || r == '\u{fffd}' || is_bidi_control(r) => {}
            r => b.push(r),
        }
    }
    b.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_bidi_control(r: char) -> bool {
    matches!(r as u32, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
}

/// The length of the escape sequence s starts with: CSI up to its final
/// byte, OSC, DCS and the like up to BEL or ST, else ESC and one more.
fn esc_len(s: &[u8]) -> usize {
    if s.len() < 2 {
        return s.len();
    }
    match s[1] {
        b'[' => {
            for (i, &c) in s.iter().enumerate().skip(2) {
                if (0x40..=0x7e).contains(&c) {
                    return i + 1;
                }
            }
            s.len()
        }
        b']' | b'P' | b'_' | b'^' | b'X' => {
            for i in 2..s.len() {
                if s[i] == 0x07 {
                    return i + 1;
                }
                if s[i] == 0x1b && i + 1 < s.len() && s[i + 1] == b'\\' {
                    return i + 2;
                }
            }
            s.len()
        }
        // ESC and one more byte, as Go has it; the rest of a character cut
        // there would be dropped as invalid, so the whole character goes.
        c => {
            let w = match c {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => 1,
            };
            (1 + w).min(s.len())
        }
    }
}

/// Checks a string's length and makes it one plain line.
pub(crate) fn clean_label(what: &str, s: &str, max: usize) -> Result<String, String> {
    let s = clean(s);
    if s.chars().count() > max {
        return Err(format!("{what} {} is over {max} characters", go_q(&s)));
    }
    Ok(s)
}

/// s as Go's %q writes it.
pub fn go_q(s: &str) -> String {
    crate::gopath::quote(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_drops_what_a_terminal_acts_on() {
        assert_eq!(printable("a\x1b[31mb\tc\u{202e}d\u{9b}"), "a[31mb cd");
        assert_eq!(clip("héllo", 3), "hé…");
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn clean_makes_one_plain_line() {
        assert_eq!(clean("Kan\x1b[31mban"), "Kanban");
        assert_eq!(clean("Fix \x1b]0;pwned\x07login\tbug"), "Fix login bug");
        assert_eq!(clean("Wait\u{202e}ing\n"), "Waiting");
        assert_eq!(clean("  a \n\n b  "), "a b");
        assert_eq!(clean("x\x1bé"), "x");
        assert_eq!(clean("\x1b"), "");
    }
}
