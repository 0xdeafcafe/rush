//! What an image in a tool result costs in tokens: its size read from the
//! first bytes of its base64 (PNG, JPEG, WebP, GIF; nothing else is
//! decoded), resized and counted by Anthropic's rule. Rust-only.
//!
//! The rule, from platform.claude.com/docs/en/build-with-claude/vision
//! ("Resolution and token cost") and vision-coordinates ("How Claude
//! resizes and pads images"), read 2026-11-09: an image costs
//! ⌈w/28⌉ × ⌈h/28⌉ visual tokens, after it is scaled down, keeping its
//! aspect, to the largest size whose padded edges fit the model's long
//! edge and whose tokens fit its budget: 2,576 px and 4,784 tokens on
//! Claude 4.7 and later (and Fable, Mythos), 1,568 px and 1,568 tokens on
//! the rest. DESIGN.md says the same. This is the one copy of the rule:
//! the context guard's copies and contact sheets (rush-bundled
//! contextguard/images.rs) count with it too.

/// One patch of an image: a visual token covers PATCH × PATCH pixels.
pub const PATCH: i64 = 28;

/// The standard tier's limits: the long edge, and the visual tokens.
pub const STANDARD_EDGE: i64 = 1_568;
pub const STANDARD_TOKENS: i64 = 1_568;
/// The high-resolution tier's (Claude 4.7 and later).
pub const HIGH_EDGE: i64 = 2_576;
pub const HIGH_TOKENS: i64 = 4_784;

/// What an image whose size can't be read is taken to cost: the standard
/// tier's budget, about a full screenshot there and a smaller one on the
/// high-resolution tier.
pub const UNREAD_TOKENS: i64 = STANDARD_TOKENS;

/// The visual tokens of an image of w × h pixels as it is sent.
pub fn patches(w: i64, h: i64) -> i64 {
    ceil_div(w, PATCH) * ceil_div(h, PATCH)
}

fn ceil_div(a: i64, b: i64) -> i64 {
    (a + b - 1) / b
}

/// The size an image of w × h is scaled to before it's counted, under a
/// long edge of max_edge and a budget of max_tokens: the docs' reference
/// implementation, its short edge rounded half to even as the API does.
pub fn resized(w: i64, h: i64, max_edge: i64, max_tokens: i64) -> (i64, i64) {
    let fits = |w: i64, h: i64| {
        ceil_div(w, PATCH) * PATCH <= max_edge
            && ceil_div(h, PATCH) * PATCH <= max_edge
            && patches(w, h) <= max_tokens
    };
    if w <= 0 || h <= 0 || fits(w, h) {
        return (w, h);
    }
    if h > w {
        let (rh, rw) = resized(h, w, max_edge, max_tokens);
        return (rw, rh);
    }
    let aspect = w as f64 / h as f64;
    let short = |long: i64| ((long as f64 / aspect).round_ties_even() as i64).max(1);
    let (mut lo, mut hi) = (1, w);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if fits(mid, short(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo, short(lo))
}

/// Whether model reads images on the high-resolution tier: Claude 4.7 and
/// later, Fable and Mythos, and a model it can't tell (the newest are).
pub fn high_res(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    if m.contains("fable") || m.contains("mythos") {
        return true;
    }
    for family in ["opus-", "sonnet-", "haiku-"] {
        let Some(i) = m.find(family) else { continue };
        let mut nums = m[i + family.len()..]
            .split('-')
            .map_while(|p| p.parse::<u32>().ok().filter(|_| p.len() <= 2));
        let major = nums.next().unwrap_or(99);
        let minor = nums.next().unwrap_or(0);
        return major > 4 || (major == 4 && minor >= 7);
    }
    true
}

/// The tokens an image of w × h costs on model.
pub fn tokens_for(w: i64, h: i64, model: &str) -> i64 {
    let (edge, budget) = if high_res(model) {
        (HIGH_EDGE, HIGH_TOKENS)
    } else {
        (STANDARD_EDGE, STANDARD_TOKENS)
    };
    let (w, h) = resized(w, h, edge, budget);
    patches(w, h)
}

/// The tokens a base64 image costs on model: UNREAD_TOKENS when its size
/// can't be read from its header.
pub fn tokens(data: &str, model: &str) -> i64 {
    match size(data) {
        Some((w, h)) => tokens_for(w, h, model),
        None => UNREAD_TOKENS,
    }
}

/// An image's width and height, from the header of its base64 (standard
/// or URL-safe), decoding only the bytes the header needs.
pub fn size(data: &str) -> Option<(i64, i64)> {
    let b = B64(data.as_bytes());
    let head = b.bytes(0, 30)?;
    let (w, h) = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(&head[16..20]), be32(&head[20..24]))
    } else if head.starts_with(b"GIF8") {
        (le16(&head[6..8]), le16(&head[8..10]))
    } else if head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        webp(&head)?
    } else if head.starts_with(b"\xff\xd8") {
        jpeg(&b)?
    } else {
        return None;
    };
    (w > 0 && h > 0).then_some((w, h))
}

fn be16(b: &[u8]) -> i64 {
    i64::from(u16::from_be_bytes([b[0], b[1]]))
}
fn le16(b: &[u8]) -> i64 {
    i64::from(u16::from_le_bytes([b[0], b[1]]))
}
fn be32(b: &[u8]) -> i64 {
    i64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
fn le24(b: &[u8]) -> i64 {
    i64::from(b[0]) | (i64::from(b[1]) << 8) | (i64::from(b[2]) << 16)
}

fn webp(h: &[u8]) -> Option<(i64, i64)> {
    match &h[12..16] {
        b"VP8 " if h[23..26] == [0x9d, 0x01, 0x2a] => {
            Some((le16(&h[26..28]) & 0x3fff, le16(&h[28..30]) & 0x3fff))
        }
        b"VP8L" if h[20] == 0x2f => {
            let bits = u32::from_le_bytes([h[21], h[22], h[23], h[24]]);
            Some((
                i64::from(bits & 0x3fff) + 1,
                i64::from((bits >> 14) & 0x3fff) + 1,
            ))
        }
        b"VP8X" => Some((le24(&h[24..27]) + 1, le24(&h[27..30]) + 1)),
        _ => None,
    }
}

/// A JPEG's size from its first frame header, walking the segments before
/// it by their lengths.
fn jpeg(b: &B64) -> Option<(i64, i64)> {
    let mut p = 2;
    for _ in 0..1024 {
        let m = b.bytes(p, 2)?;
        if m[0] != 0xff {
            return None;
        }
        let marker = m[1];
        match marker {
            0xff => {
                p += 1; // fill
                continue;
            }
            0x01 | 0xd0..=0xd7 => {
                p += 2;
                continue;
            }
            0xd9 | 0xda => return None,
            _ => {}
        }
        let seg = b.bytes(p + 2, 2)?;
        let len = be16(&seg) as usize;
        let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if sof {
            let f = b.bytes(p + 5, 4)?;
            return Some((be16(&f[2..4]), be16(&f[0..2])));
        }
        if len < 2 {
            return None;
        }
        p += 2 + len;
    }
    None
}

/// Base64 text, read at any byte of what it decodes to.
struct B64<'a>(&'a [u8]);

fn sextet(c: u8) -> Option<u32> {
    Some(u32::from(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    }))
}

impl B64<'_> {
    /// n decoded bytes from off, or None past the end. Each four
    /// characters are three bytes, so only the groups holding them are
    /// decoded.
    fn bytes(&self, off: usize, n: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(n + 3);
        let mut g = off / 3;
        let skip = off % 3;
        while out.len() < skip + n {
            let chunk = self.0.get(g * 4..g * 4 + 4)?;
            let mut v = 0u32;
            let mut got = 0;
            for &c in chunk {
                if c == b'=' {
                    break;
                }
                v = (v << 6) | sextet(c)?;
                got += 1;
            }
            if got < 2 {
                return None;
            }
            v <<= 6 * (4 - got);
            let all = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
            out.extend_from_slice(&all[..got - 1]);
            if got < 4 {
                break;
            }
            g += 1;
        }
        (out.len() >= skip + n).then(|| out[skip..skip + n].to_vec())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Standard base64 of b: for building images' headers in tests.
    pub fn b64(b: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let v = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    s.push(A[((v >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    /// A PNG's first bytes, enough for its size, and some after.
    pub fn png(w: u32, h: u32) -> String {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0, 0]);
        b64(&b)
    }

    fn jpeg(w: u16, h: u16) -> String {
        let mut b = vec![0xff, 0xd8];
        // An APP0 segment to walk past, then a fill byte.
        b.extend([0xff, 0xe0, 0, 16]);
        b.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        b.extend([0xff, 0xff, 0xc2, 0, 17, 8]);
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        b64(&b)
    }

    fn webp(chunk: &[u8; 4], body: &[u8]) -> String {
        let mut b = b"RIFF\0\0\0\0WEBP".to_vec();
        b.extend(chunk);
        b.extend([0, 0, 0, 0]);
        b.extend(body);
        b.resize(40, 0);
        b64(&b)
    }

    #[test]
    fn sizes_from_headers() {
        let gif = b64(b"GIF89a\x40\x01\xf0\x00\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let vp8 = webp(
            b"VP8 ",
            &[0, 0, 0, 0x9d, 0x01, 0x2a, 0x80, 0x07, 0x38, 0x04],
        );
        // VP8L: 0x2f, then width-1 and height-1 in 14 bits each.
        let bits: u32 = 799 | (599 << 14);
        let mut l = vec![0x2f];
        l.extend(bits.to_le_bytes());
        let vp8l = webp(b"VP8L", &l);
        let vp8x = webp(b"VP8X", &[0, 0, 0, 0, 0xff, 0x0e, 0, 0x6f, 0x08, 0]);
        for (name, data, want) in [
            ("png", png(1920, 1080), Some((1920, 1080))),
            ("jpeg", jpeg(4032, 3024), Some((4032, 3024))),
            ("gif", gif, Some((320, 240))),
            ("webp vp8", vp8, Some((1920, 1080))),
            ("webp vp8l", vp8l, Some((800, 600))),
            ("webp vp8x", vp8x, Some((3840, 2160))),
            (
                "not an image",
                b64(b"hello, this is only some text!!"),
                None,
            ),
            ("too short", png(10, 10)[..12].to_string(), None),
            ("not base64", "!!!!".repeat(20), None),
        ] {
            assert_eq!(size(&data), want, "{name}");
        }
    }

    // The docs' own examples, on each tier.
    #[test]
    fn tokens_by_the_docs() {
        for (w, h, std_tok, high_tok) in [
            (200, 200, 64, 64),
            (1000, 1000, 1296, 1296),
            (1092, 1092, 1521, 1521),
            (1920, 1080, 1560, 2691),
            (2000, 1500, 1564, 3888),
            (3840, 2160, 1560, 4784),
        ] {
            assert_eq!(tokens_for(w, h, "claude-haiku-4-5"), std_tok, "{w}x{h}");
            assert_eq!(tokens_for(w, h, "claude-opus-5-5"), high_tok, "{w}x{h}");
        }
        assert_eq!(resized(1075, 1520, 1568, 1568), (924, 1307));
        assert_eq!(resized(1920, 1080, 1568, 1568), (1456, 819));
        assert_eq!(resized(3840, 2160, 2576, 4784), (2576, 1449));
    }

    #[test]
    fn tiers() {
        for (model, high) in [
            ("claude-opus-5-5", true),
            ("claude-sonnet-5", true),
            ("claude-opus-4-7", true),
            ("claude-opus-4-6", false),
            ("claude-sonnet-4-5-20250929", false),
            ("claude-haiku-4-5-20251001", false),
            ("claude-fable-5-1", true),
            ("", true),
        ] {
            assert_eq!(high_res(model), high, "{model}");
        }
    }

    #[test]
    fn unread_images_cost_the_named_guess() {
        assert_eq!(tokens("not an image", "claude-opus-5-5"), UNREAD_TOKENS);
        assert_eq!(tokens(&png(200, 200), "claude-opus-5-5"), 64);
    }
}
