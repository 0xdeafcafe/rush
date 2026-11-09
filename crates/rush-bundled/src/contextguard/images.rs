//! Images an agent reads, made cheaper: a WebP copy no bigger than the
//! model needs, and many images tiled onto one captioned contact sheet.
//! Anthropic bills an image by its pixels, not its bytes: ⌈w/28⌉ × ⌈h/28⌉
//! visual tokens after the model's own resize (platform.claude.com, Vision
//! › Resolution and token cost), so a copy with fewer pixels is what saves
//! tokens; WebP saves bytes, the transcript's size and upload time.
//! Nothing here ever breaks a read: whatever can't be read, decoded or
//! encoded gives None, and the read goes ahead as asked. Rust-only.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits};

/// One visual token per patch this many pixels square.
pub const PATCH: u32 = 28;
/// The high-resolution tier's limits (Claude 4.7 and later): what a copy
/// that isn't made would be cut to by the model itself.
pub const MODEL_EDGE: u32 = 2576;
pub const MODEL_TOKENS: u32 = 4784;
/// The standard tier's long edge, the edge a copy is cut to at budgets up
/// to its tokens.
pub const STANDARD_EDGE: u32 = 1568;
/// The guard's own budget for one image, unless set: the standard tier's,
/// under a third of what the high-resolution tier spends on a big
/// screenshot.
pub const TOKENS: u32 = 1568;
/// An image this small in bytes, and in budget already, is read as it is.
pub const SMALL_IMAGE: u64 = 64 * 1024;
/// libwebp's quality, 0-100: text in screenshots stays sharp.
pub const QUALITY: f32 = 85.0;
/// The most images one sheet takes.
pub const SHEET_MAX: usize = 48;
/// Where copies and sheets are kept, under the agent's scratch folder.
pub const DIR: &str = "rush-images";
/// Decoding gives up past this much memory.
const MAX_ALLOC: u64 = 512 << 20;

/// An image's visual tokens as it's sent.
pub fn tokens(w: u32, h: u32) -> u64 {
    u64::from(w.div_ceil(PATCH)) * u64::from(h.div_ceil(PATCH))
}

/// The long edge a budget of tokens goes with.
pub fn edge_for(budget: u32) -> u32 {
    if budget <= TOKENS {
        STANDARD_EDGE
    } else {
        MODEL_EDGE
    }
}

/// The largest size, of the same shape, that keeps within max_edge and
/// max_tokens: the model's own resize, ported from the reference in
/// platform.claude.com, Vision coordinates › Resize your image before
/// uploading. An image that fits is its own size.
pub fn resized(w: u32, h: u32, max_edge: u32, max_tokens: u32) -> (u32, u32) {
    let fits = |w: u32, h: u32| {
        w.div_ceil(PATCH) * PATCH <= max_edge
            && h.div_ceil(PATCH) * PATCH <= max_edge
            && tokens(w, h) <= u64::from(max_tokens)
    };
    if w == 0 || h == 0 || fits(w, h) {
        return (w, h);
    }
    if h > w {
        let (b, a) = resized(h, w, max_edge, max_tokens);
        return (a, b);
    }
    let aspect = f64::from(w) / f64::from(h);
    let short = |long: u32| ((f64::from(long) / aspect).round_ties_even() as u32).max(1);
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

/// The tokens an image of w × h costs as it is, on the high-resolution
/// tier: after the model's own resize.
pub fn billed(w: u32, h: u32) -> u64 {
    let (a, b) = resized(w, h, MODEL_EDGE, MODEL_TOKENS);
    tokens(a, b)
}

/// Whether Claude Code's Read shows path as an image, by its name.
pub fn is_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

fn reader(path: &Path) -> Option<ImageReader<std::io::BufReader<fs::File>>> {
    let mut r = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    r.limits(limits);
    Some(r)
}

/// An image's width and height, from its header alone.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    reader(path)?.into_dimensions().ok()
}

/// An image decoded, turned as its EXIF says (a phone's photos).
fn decode(path: &Path) -> Option<DynamicImage> {
    let mut d = reader(path)?.into_decoder().ok()?;
    let turn = d.orientation().ok();
    let mut img = DynamicImage::from_decoder(d).ok()?;
    if let Some(o) = turn {
        img.apply_orientation(o);
    }
    Some(img)
}

/// img as lossy WebP; None when libwebp can't (or panics).
pub fn webp(img: &DynamicImage) -> Option<Vec<u8>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        // An alpha channel that's opaque throughout (most screenshots')
        // costs bytes for nothing.
        let alpha = img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] != 255);
        let out = if alpha {
            let px = img.to_rgba8();
            webp::Encoder::from_rgba(px.as_raw(), w, h).encode_simple(false, QUALITY)
        } else {
            let px = img.to_rgb8();
            webp::Encoder::from_rgb(px.as_raw(), w, h).encode_simple(false, QUALITY)
        };
        out.ok().map(|m| m.to_vec())
    }))
    .ok()
    .flatten()
}

/// A smaller copy made of an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shrunk {
    pub path: PathBuf,
    /// Its width and height, and the copy's.
    pub from: (u32, u32),
    pub to: (u32, u32),
    /// Its bytes, and the copy's.
    pub before: u64,
    pub after: u64,
}

impl Shrunk {
    /// The tokens the original costs, and the copy.
    pub fn tokens(&self) -> (u64, u64) {
        (
            billed(self.from.0, self.from.1),
            tokens(self.to.0, self.to.1),
        )
    }
}

/// A WebP copy of src in dir, cut to budget's tokens, for the agent to
/// read instead; None when it wouldn't help (small already, or a WebP that
/// fits) or can't be made. Made once per file as it is: asked again, the
/// copy already there is given.
pub fn shrink(src: &Path, dir: &Path, budget: u32) -> Option<Shrunk> {
    let md = fs::metadata(src).ok()?;
    let before = md.len();
    let r = reader(src)?;
    let format = r.format();
    let from = r.into_dimensions().ok()?;
    let edge = edge_for(budget);
    let fits = resized(from.0, from.1, edge, budget) == from;
    if fits && (before < SMALL_IMAGE || format == Some(ImageFormat::WebP)) {
        return None;
    }
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let key =
        super::hook::fnv(format!("{}\0{before}\0{mtime}\0{budget}", src.display()).as_bytes());
    let out = dir.join(format!("{key:016x}.webp"));
    if let Ok(m) = fs::metadata(&out)
        && m.len() > 0
        && let Some(to) = dimensions(&out)
    {
        // Turned as its EXIF said, the copy may stand the other way.
        let from = if (to.0 > to.1) == (from.0 > from.1) {
            from
        } else {
            (from.1, from.0)
        };
        return Some(Shrunk {
            path: out,
            from,
            to,
            before,
            after: m.len(),
        });
    }
    let img = decode(src)?;
    let (w, h) = img.dimensions();
    let to = resized(w, h, edge, budget);
    let img = if to == (w, h) {
        img
    } else {
        img.resize_exact(to.0, to.1, FilterType::CatmullRom)
    };
    let b = webp(&img)?;
    // The same pixels are worth a copy only for a quarter fewer bytes.
    if to == (w, h) && b.len() as u64 * 4 > before * 3 {
        return None;
    }
    if !keep(&out, &b) {
        return None;
    }
    Some(Shrunk {
        path: out,
        from: (w, h),
        to,
        before,
        after: b.len() as u64,
    })
}

/// Writes b to path whole (a reader never sees half of it), for the agent
/// alone.
pub fn keep(path: &Path, b: &[u8]) -> bool {
    if let Some(d) = path.parent()
        && fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(d)
            .is_err()
    {
        return false;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let ok = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(b))
        .is_ok();
    if ok && fs::rename(&tmp, path).is_ok() {
        return true;
    }
    let _ = fs::remove_file(&tmp);
    false
}

/// A contact sheet: many images on one, each captioned.
#[derive(Clone, Debug)]
pub struct Sheet {
    /// Lossy WebP.
    pub webp: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Images across.
    pub columns: u32,
    /// How many are on it, read or not.
    pub count: usize,
    /// The ones that couldn't be read: their cells say so.
    pub unread: Vec<String>,
}

impl Sheet {
    pub fn tokens(&self) -> u64 {
        tokens(self.width, self.height)
    }
}

/// Space between cells and around the sheet.
const GAP: u32 = 6;
const BACKGROUND: [u8; 3] = [32, 32, 36];
const CELL: [u8; 3] = [48, 48, 54];
const INK: [u8; 3] = [236, 236, 236];
const FAINT: [u8; 3] = [150, 150, 158];

/// The most columns of images of shape aspect, of n, that fill a sheet in
/// budget; (columns, each image's box width and height).
fn layout(n: u32, aspect: f64, budget: u32, caption: u32) -> (u32, u32, u32) {
    let edge = edge_for(budget);
    let mut best = (1, 0, 0);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let size = |x: u32| {
            let h = ((f64::from(x) / aspect).round() as u32).max(1);
            (cols * (x + GAP) + GAP, rows * (h + caption + GAP) + GAP, h)
        };
        let fits = |x: u32| {
            let (w, h, _) = size(x);
            resized(w, h, edge, budget) == (w, h)
        };
        if !fits(1) {
            continue;
        }
        let (mut lo, mut hi) = (1, edge);
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if lo > best.1 {
            best = (cols, lo, size(lo).2);
        }
    }
    best
}

/// A caption that fits in max characters: its middle left out (an id or a
/// number is at the end of most names).
pub fn fit_caption(s: &str, max: usize) -> String {
    let chars: Vec<char> = s
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '?'
            }
        })
        .collect();
    if chars.len() <= max || max < 3 {
        return chars.into_iter().take(max).collect();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('~');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Writes text at (x, y), font8x8's glyphs at scale.
fn draw_text(img: &mut image::RgbImage, x: u32, y: u32, text: &str, scale: u32, ink: [u8; 3]) {
    let (w, h) = img.dimensions();
    for (i, c) in text.chars().enumerate() {
        let glyph = font8x8::legacy::BASIC_LEGACY[(c as usize).min(127)];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (bits >> col) & 1 == 0 {
                    continue;
                }
                let px = x + (i as u32 * 8 + col) * scale;
                let py = y + row as u32 * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        if px + dx < w && py + dy < h {
                            img.put_pixel(px + dx, py + dy, image::Rgb(ink));
                        }
                    }
                }
            }
        }
    }
}

fn fill(img: &mut image::RgbImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, image::Rgb(c));
        }
    }
}

/// Tiles images onto one sheet within budget's tokens, in the order
/// given, each under a caption of its number and file name. Err says why
/// there's no sheet at all.
pub fn sheet(paths: &[PathBuf], budget: u32) -> Result<Sheet, String> {
    if paths.is_empty() {
        return Err("No images: give the paths of the images to put on the sheet.".into());
    }
    if paths.len() > SHEET_MAX {
        return Err(format!(
            "That's {} images; a sheet takes {SHEET_MAX} at most, and fewer are easier to see. Make one sheet per {} or so.",
            paths.len(),
            SHEET_MAX / 3
        ));
    }
    let dims: Vec<Option<(u32, u32)>> = paths.iter().map(|p| dimensions(p)).collect();
    let mut aspects: Vec<f64> = dims
        .iter()
        .flatten()
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map(|(w, h)| f64::from(*w) / f64::from(*h))
        .collect();
    if aspects.is_empty() {
        return Err("None of those could be read as an image (PNG, JPEG, GIF or WebP).".into());
    }
    aspects.sort_by(f64::total_cmp);
    let aspect = aspects[aspects.len() / 2].clamp(0.25, 4.0);
    let n = paths.len() as u32;
    // Captions at twice the font's size when the cells are wide enough.
    let mut scale = 2;
    let mut lay = layout(n, aspect, budget, 8 * scale + 6);
    if lay.1 < 320 {
        scale = 1;
        lay = layout(n, aspect, budget, 8 * scale + 6);
    }
    let (cols, bw, bh) = lay;
    if bw == 0 {
        return Err("Those don't fit on one sheet; make one per few images.".into());
    }
    let caption = 8 * scale + 6;
    let rows = n.div_ceil(cols);
    let (width, height) = (cols * (bw + GAP) + GAP, rows * (bh + caption + GAP) + GAP);
    let mut canvas = image::RgbImage::from_pixel(width, height, image::Rgb(BACKGROUND));
    let max_chars = (bw / (8 * scale)) as usize;
    let mut unread = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let x = GAP + c * (bw + GAP);
        let y = GAP + r * (bh + caption + GAP);
        let name = p
            .file_name()
            .map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy())
            .into_owned();
        let img = dims[i].and_then(|_| decode(p));
        let label = if img.is_some() {
            format!("{}. {name}", i + 1)
        } else {
            unread.push(p.to_string_lossy().into_owned());
            format!("{}. {name} (can't read)", i + 1)
        };
        draw_text(
            &mut canvas,
            x,
            y + 2,
            &fit_caption(&label, max_chars),
            scale,
            if img.is_some() { INK } else { FAINT },
        );
        let top = y + caption - 2;
        fill(&mut canvas, x, top, bw, bh, CELL);
        if let Some(img) = img {
            let thumb = img.resize(bw, bh, FilterType::CatmullRom).to_rgb8();
            let (tw, th) = thumb.dimensions();
            image::imageops::overlay(
                &mut canvas,
                &thumb,
                i64::from(x + (bw - tw.min(bw)) / 2),
                i64::from(top + (bh - th.min(bh)) / 2),
            );
        }
    }
    let webp = webp(&DynamicImage::ImageRgb8(canvas))
        .ok_or_else(|| "The sheet couldn't be encoded as WebP.".to_string())?;
    Ok(Sheet {
        webp,
        width,
        height,
        columns: cols,
        count: paths.len(),
        unread,
    })
}

/// Where a sheet of paths is kept, unless told: in tmp's rush-images, by
/// the images and when each last changed, so the same sheet is the same
/// file.
pub fn sheet_path(tmp: &Path, paths: &[PathBuf]) -> PathBuf {
    let mut key = String::new();
    for p in paths {
        let t = fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        key.push_str(&format!("{}\0{t}\0", p.display()));
    }
    tmp.join(DIR).join(format!(
        "sheet-{:016x}.webp",
        super::hook::fnv(key.as_bytes())
    ))
}

/// A count with thousands separated: 1,560.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a sheet made says, in the tool's answer and the command's.
pub fn sheet_said(s: &Sheet, at: &Path) -> String {
    let mut out = format!(
        "{} {} on one sheet, {} across: {}×{}, about {} tokens. Kept at {}.",
        s.count,
        if s.count == 1 { "image" } else { "images" },
        s.columns,
        s.width,
        s.height,
        thousands(s.tokens()),
        at.display()
    );
    if !s.unread.is_empty() {
        out.push_str(&format!(" Couldn't read: {}.", s.unread.join(", ")));
    }
    out
}

/// The budget a sheet or copy keeps to: the context guard's, as set.
pub fn budget() -> u32 {
    let r = super::rules();
    if r.image_tokens == 0 {
        TOKENS
    } else {
        r.image_tokens
    }
}

pub const SHEET_USAGE: &str = "\
usage: rush contact-sheet [-o out.webp] [--tokens N] <image>…
  tiles the images onto one WebP, each captioned with its number and file
  name, sized to cost an agent at most about N tokens (the context guard's
  image budget unless given; 1,568 by default)
";

/// rush contact-sheet: the sheet made, and where it's kept, said.
pub fn sheet_cmd(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut paths = Vec::new();
    let mut to: Option<PathBuf> = None;
    let mut tokens = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" | "help" => {
                let _ = out.write_all(SHEET_USAGE.as_bytes());
                return 0;
            }
            "-o" | "--out" => match it.next() {
                Some(p) => to = Some(PathBuf::from(p)),
                None => {
                    let _ = write!(err, "rush contact-sheet: -o needs a file\n{SHEET_USAGE}");
                    return 2;
                }
            },
            "--tokens" => match it
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n >= 64)
            {
                Some(n) => tokens = Some(n),
                None => {
                    let _ = write!(
                        err,
                        "rush contact-sheet: --tokens needs a number, 64 or more\n{SHEET_USAGE}"
                    );
                    return 2;
                }
            },
            _ => paths.push(PathBuf::from(a)),
        }
    }
    if paths.is_empty() {
        let _ = err.write_all(SHEET_USAGE.as_bytes());
        return 2;
    }
    let s = match sheet(&paths, tokens.unwrap_or_else(budget)) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "rush contact-sheet: {e}");
            return 1;
        }
    };
    let at = to.unwrap_or_else(|| sheet_path(&std::env::temp_dir(), &paths));
    if !keep(&at, &s.webp) {
        let _ = writeln!(err, "rush contact-sheet: couldn't write {}", at.display());
        return 1;
    }
    let _ = writeln!(out, "{}", sheet_said(&s, &at));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizes_as_the_model_does() {
        // The examples in platform.claude.com's Vision pages.
        // Width, height, the long edge and tokens allowed; the size and its tokens.
        type Case = (u32, u32, u32, u32, (u32, u32), u64);
        let cases: &[Case] = &[
            (200, 200, 1568, 1568, (200, 200), 64),
            (1000, 1000, 1568, 1568, (1000, 1000), 1296),
            (1092, 1092, 1568, 1568, (1092, 1092), 1521),
            (1920, 1080, 1568, 1568, (1456, 819), 1560),
            // The pages' table says 1269×952; their reference code, ported
            // here, gives 1270 (both cost 1,564).
            (2000, 1500, 1568, 1568, (1270, 952), 1564),
            (3840, 2160, 1568, 1568, (1456, 819), 1560),
            (1075, 1520, 1568, 1568, (924, 1307), 1551),
            (1920, 1080, 2576, 4784, (1920, 1080), 2691),
            (3840, 2160, 2576, 4784, (2576, 1449), 4784),
        ];
        for &(w, h, e, t, want, _) in cases {
            assert_eq!(resized(w, h, e, t), want, "{w}x{h} in {e}/{t}");
        }
        for &(w, h, e, t, _, toks) in cases {
            let (a, b) = resized(w, h, e, t);
            assert_eq!(tokens(a, b), toks, "{w}x{h}");
        }
        assert_eq!(billed(1920, 1080), 2691);
        assert_eq!(billed(3840, 2160), 4784);
    }

    #[test]
    fn captions_fit() {
        let cases: &[(&str, usize, &str)] = &[
            ("1. a.png", 20, "1. a.png"),
            ("12. screenshot-of-the-page-03.png", 15, "12. scr~-03.png"),
            ("1. é.png", 20, "1. ?.png"),
            ("abc", 2, "ab"),
        ];
        for &(s, max, want) in cases {
            assert_eq!(fit_caption(s, max), want, "{s} in {max}");
        }
    }

    #[test]
    fn thousands_separated() {
        for (n, want) in [
            (0, "0"),
            (999, "999"),
            (1560, "1,560"),
            (1234567, "1,234,567"),
        ] {
            assert_eq!(thousands(n), want);
        }
    }

    /// A deterministic run of numbers, for fixtures.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    /// Lines of code-like text from (x, y), each line's length random.
    #[allow(clippy::too_many_arguments)]
    fn text_block(
        img: &mut image::RgbImage,
        rng: &mut Lcg,
        x: u32,
        y: u32,
        w: u32,
        lines: u32,
        scale: u32,
        ink: [u8; 3],
    ) {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz    ;(){}=._-ABCDEFG0123456789";
        let per = (w / (8 * scale)) as usize;
        for l in 0..lines {
            let n = (rng.next() as usize % per.max(1)).max(4);
            let s: String = (0..n)
                .map(|_| CHARS[rng.next() as usize % CHARS.len()] as char)
                .collect();
            draw_text(img, x, y + l * 12 * scale, &s, scale, ink);
        }
    }

    /// Representative screenshots, made here: (what, file name, bytes).
    fn fixtures() -> Vec<(&'static str, &'static str, Vec<u8>)> {
        let mut rng = Lcg(7);
        let png = |img: image::RgbImage| {
            let mut b = std::io::Cursor::new(Vec::new());
            DynamicImage::ImageRgb8(img)
                .write_to(&mut b, ImageFormat::Png)
                .unwrap();
            b.into_inner()
        };
        // A Retina terminal: light text on dark, the whole screen.
        let mut term = image::RgbImage::from_pixel(3024, 1964, image::Rgb([30, 30, 34]));
        text_block(&mut term, &mut rng, 24, 24, 2976, 80, 2, [210, 210, 200]);
        // A web page at 1080p: a header, a sidebar, body text, buttons.
        let mut page = image::RgbImage::from_pixel(1920, 1080, image::Rgb([255, 255, 255]));
        fill(&mut page, 0, 0, 1920, 64, [36, 92, 200]);
        fill(&mut page, 0, 64, 280, 1016, [244, 245, 247]);
        text_block(&mut page, &mut rng, 24, 96, 240, 40, 1, [60, 60, 70]);
        text_block(&mut page, &mut rng, 320, 96, 1560, 36, 2, [20, 20, 24]);
        for i in 0..4 {
            fill(&mut page, 320 + i * 180, 980, 160, 44, [36, 160, 90]);
        }
        // A phone, portrait: cards of text.
        let mut phone = image::RgbImage::from_pixel(1179, 2556, image::Rgb([242, 242, 247]));
        for i in 0..8 {
            fill(&mut phone, 40, 160 + i * 290, 1099, 250, [255, 255, 255]);
            text_block(
                &mut phone,
                &mut rng,
                64,
                184 + i * 290,
                1040,
                6,
                3,
                [28, 28, 30],
            );
        }
        // A photo: gradients and grain, as JPEG.
        let mut photo = image::RgbImage::new(2000, 1500);
        for (x, y, p) in photo.enumerate_pixels_mut() {
            let n = (rng.next() % 24) as u8;
            *p = image::Rgb([
                ((x * 255 / 2000) as u8).saturating_add(n),
                ((y * 255 / 1500) as u8).saturating_add(n),
                (128u8).saturating_add(n),
            ]);
        }
        let mut jpeg = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(photo)
            .write_to(&mut jpeg, ImageFormat::Jpeg)
            .unwrap();
        // An icon: small, left alone.
        let icon = image::RgbImage::from_pixel(128, 128, image::Rgb([200, 40, 40]));
        vec![
            ("Retina terminal, 3024×1964 PNG", "terminal.png", png(term)),
            ("web page, 1920×1080 PNG", "page.png", png(page)),
            ("phone, 1179×2556 PNG", "phone.png", png(phone)),
            ("photo, 2000×1500 JPEG", "photo.jpg", jpeg.into_inner()),
            ("icon, 128×128 PNG", "icon.png", png(icon)),
        ]
    }

    /// Before and after, bytes and tokens (tokens as the high-resolution
    /// tier bills the original), for DESIGN.md's table.
    #[test]
    fn measured_on_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let copies = dir.path().join(DIR);
        let mut got = Vec::new();
        for (what, name, bytes) in fixtures() {
            let p = dir.path().join(name);
            fs::write(&p, &bytes).unwrap();
            let (w, h) = dimensions(&p).unwrap();
            match shrink(&p, &copies, TOKENS) {
                Some(s) => {
                    let (tb, ta) = s.tokens();
                    assert!(ta <= u64::from(TOKENS), "{what}: {ta} tokens");
                    let c = fs::read(&s.path).unwrap();
                    assert!(c.starts_with(b"RIFF") && &c[8..12] == b"WEBP", "{what}");
                    assert_eq!(dimensions(&s.path), Some(s.to), "{what}");
                    assert_eq!(c.len() as u64, s.after, "{what}");
                    // Asked again, the same copy.
                    assert_eq!(shrink(&p, &copies, TOKENS).as_ref(), Some(&s), "{what}");
                    got.push((what, s.before, s.after, tb, ta));
                }
                None => got.push((
                    what,
                    bytes.len() as u64,
                    bytes.len() as u64,
                    billed(w, h),
                    billed(w, h),
                )),
            }
        }
        let want: Vec<(&str, u64, u64, u64, u64)> = vec![
            ("Retina terminal, 3024×1964 PNG", 0, 0, 0, 0),
            ("web page, 1920×1080 PNG", 0, 0, 0, 0),
            ("phone, 1179×2556 PNG", 0, 0, 0, 0),
            ("photo, 2000×1500 JPEG", 0, 0, 0, 0),
            ("icon, 128×128 PNG", 0, 0, 0, 0),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn leaves_alone_what_wouldnt_help() {
        let dir = tempfile::tempdir().unwrap();
        let copies = dir.path().join(DIR);
        let small = dir.path().join("small.png");
        image::RgbImage::from_pixel(400, 300, image::Rgb([1, 2, 3]))
            .save(&small)
            .unwrap();
        let fake = dir.path().join("fake.png");
        fs::write(&fake, vec![b'x'; 200_000]).unwrap();
        let webp_fits = dir.path().join("fits.webp");
        let mut big = image::RgbImage::new(1000, 700);
        let mut rng = Lcg(3);
        for p in big.pixels_mut() {
            *p = image::Rgb([rng.next() as u8, rng.next() as u8, rng.next() as u8]);
        }
        fs::write(&webp_fits, webp(&DynamicImage::ImageRgb8(big)).unwrap()).unwrap();
        for p in [&small, &fake, &webp_fits, &dir.path().join("missing.png")] {
            assert_eq!(shrink(p, &copies, TOKENS), None, "{}", p.display());
        }
    }

    #[test]
    fn sheets_fit_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..12u32 {
            let p = dir.path().join(format!("shot-{i:02}.png"));
            let mut img = image::RgbImage::from_pixel(1440, 900, image::Rgb([250, 250, 250]));
            fill(&mut img, 0, 0, 1440, 60, [(i * 20) as u8, 90, 200]);
            img.save(&p).unwrap();
            paths.push(p);
        }
        paths.push(dir.path().join("gone.png"));
        let cases: &[(usize, u32)] = &[
            (1, TOKENS),
            (4, TOKENS),
            (13, TOKENS),
            (13, 800),
            (13, 3000),
        ];
        for &(n, budget) in cases {
            let s = sheet(&paths[paths.len() - n..], budget).unwrap();
            assert!(
                s.tokens() <= u64::from(budget),
                "{n} in {budget}: {}",
                s.tokens()
            );
            assert!(s.width <= edge_for(budget) && s.height <= edge_for(budget));
            assert_eq!(s.count, n);
            assert!(s.webp.starts_with(b"RIFF"));
            let at = sheet_path(dir.path(), &paths[paths.len() - n..]);
            assert!(keep(&at, &s.webp));
            assert_eq!(dimensions(&at), Some((s.width, s.height)));
            assert_eq!(s.unread.len(), 1, "gone.png is last");
        }
        let all = sheet(&paths[..12], TOKENS).unwrap();
        assert!(all.unread.is_empty() && all.columns > 1);
        assert!(sheet_said(&all, Path::new("/t/s.webp")).starts_with("12 images on one sheet, "));
        assert!(sheet(&[], TOKENS).is_err());
        let many: Vec<PathBuf> = std::iter::repeat_n(paths[0].clone(), SHEET_MAX + 1).collect();
        assert!(sheet(&many, TOKENS).is_err());
        assert!(sheet(&[dir.path().join("gone.png")], TOKENS).is_err());
    }

    #[test]
    fn the_command() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        image::RgbImage::from_pixel(800, 600, image::Rgb([9, 9, 9]))
            .save(&a)
            .unwrap();
        let out = dir.path().join("sheet.webp");
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cases: Vec<(Vec<String>, i32, &str)> = vec![
            // --tokens given: rush's own settings aren't read in a test.
            (
                s(&[
                    "-o",
                    &out.to_string_lossy(),
                    "--tokens",
                    "1568",
                    &a.to_string_lossy(),
                ]),
                0,
                "1 image on one sheet, 1 across",
            ),
            (s(&[]), 2, "usage: rush contact-sheet"),
            (s(&["-o"]), 2, "-o needs a file"),
            (s(&["--tokens", "x", "a.png"]), 2, "--tokens needs a number"),
            (s(&["--help"]), 0, "usage: rush contact-sheet"),
            (
                s(&[
                    "--tokens",
                    "800",
                    &dir.path().join("none.png").to_string_lossy(),
                ]),
                1,
                "None of those could be read",
            ),
        ];
        for (args, code, said) in cases {
            let (mut o, mut e) = (Vec::new(), Vec::new());
            assert_eq!(sheet_cmd(&args, &mut o, &mut e), code, "{args:?}");
            let both = format!(
                "{}{}",
                String::from_utf8_lossy(&o),
                String::from_utf8_lossy(&e)
            );
            assert!(both.contains(said), "{args:?}: {both}");
        }
        assert!(fs::read(&out).unwrap().starts_with(b"RIFF"));
    }
}
