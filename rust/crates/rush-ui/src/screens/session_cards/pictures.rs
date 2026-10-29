//! Pictures in coloured quarter blocks: an image fitted to a box of cells,
//! its shape kept by the cell's size, each made once off the UI thread
//! and read without decoding after. After Go's convo/picture.go (Picture
//! :51, PictureDrawn :58, drawPicture :124, Tile :222), convo/thumb.go
//! (quadGlyphs :102, flatDist :106, quadrants :118, quadCell :150,
//! thumbSlots :54, Preview :93, readThumb :190, shrinkTo :225, halfBlocks
//! :261, sameish :295) and photon's termimg (Cell, Fit).
//!
//! Gap (pictures.go): Kitty pictures, and asking the terminal its cell
//! size, wait on rush-draw writing raw output; every terminal gets the
//! blocks Go draws where Kitty isn't there, on Go's 10×20 cell.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use image::imageops::FilterType;
use rush_core::text::{Color, Line, Span, Style};

use crate::model::Model;
use crate::worker::Note;

/// A cell's size in pixels when the terminal hasn't said (termimg.Cell).
const CELL: (usize, usize) = (10, 20);

/// The most pixels decoded to draw a terminal's worth of an image.
const MOST_PIXELS: u64 = 64 << 20;

/// The most bytes read of a file to draw it (drawPicture).
const MOST_BYTES: u64 = 64 << 20;

/// Pictures kept before they're all made again as they're drawn.
const KEEP: usize = 512;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: String,
    w: usize,
    h: usize,
}

/// None while it's being made; Some(empty) when it can't be read.
type Slot = Option<Arc<Vec<Line>>>;

fn cache() -> &'static Mutex<HashMap<Key, Slot>> {
    static C: OnceLock<Mutex<HashMap<Key, Slot>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Only a couple made at once (thumbSlots): a message of many
/// screenshots asks for them all.
fn slots() -> &'static (Mutex<usize>, Condvar) {
    static S: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(2), Condvar::new()))
}

fn with_slot<T>(f: impl FnOnce() -> T) -> T {
    let (n, cv) = slots();
    let mut free = n.lock().unwrap_or_else(|e| e.into_inner());
    while *free == 0 {
        free = cv.wait(free).unwrap_or_else(|e| e.into_inner());
    }
    *free -= 1;
    drop(free);
    let v = f();
    *n.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    cv.notify_one();
    v
}

/// The image at path fitted to w cells by h rows: its rows once made
/// (none when it can't be read), None until then. It never decodes on
/// the UI thread: the first ask starts the making, and the frame after
/// it's made has it. With no loop (tests, --render) it's made at once.
pub fn picture(m: &Model, path: &str, w: usize, h: usize) -> Option<Arc<Vec<Line>>> {
    let k = Key {
        path: path.to_string(),
        w,
        h,
    };
    {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = c.get(&k) {
            return s.clone();
        }
        if c.len() >= KEEP {
            c.clear(); // made again as they're drawn
        }
        c.insert(k.clone(), None);
    }
    let Some(tx) = m.sender() else {
        let rows = Arc::new(draw_picture(path, w, h));
        put(k, rows.clone());
        return Some(rows);
    };
    let p = path.to_string();
    let made = std::thread::Builder::new()
        .name("rush-picture".into())
        .spawn(move || {
            let rows = with_slot(|| draw_picture(&p, w, h));
            put(k, Arc::new(rows));
            tx.lane(1)
                .send(Note::Apply(Box::new(|_| crate::frame::Out::Draw)));
        });
    if made.is_err() {
        put(
            Key {
                path: path.to_string(),
                w,
                h,
            },
            Arc::new(Vec::new()),
        );
    }
    None
}

fn put(k: Key, rows: Arc<Vec<Line>>) {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(k, Some(rows));
}

/// The cells a picture of pw×ph pixels takes in at most cols×rows, its
/// shape kept by the cell's (termimg.Fit).
pub fn fit(pw: usize, ph: usize, max_cols: usize, max_rows: usize) -> (usize, usize) {
    let (cw, ch) = CELL;
    if pw == 0 || ph == 0 || max_cols == 0 || max_rows == 0 {
        return (0, 0);
    }
    let (max_cols, max_rows) = (max_cols.min(297), max_rows.min(297));
    let (mut w, mut h) = (pw.min(max_cols * cw), ph.min(max_rows * ch));
    if w * ph > h * pw {
        w = h * pw / ph;
    } else {
        h = w * ph / pw;
    }
    (
        max_cols.min((w + cw / 2) / cw).max(1),
        max_rows.min((h + ch / 2) / ch).max(1),
    )
}

/// Reads, decodes and draws a picture: for off the UI thread
/// (drawPicture, in blocks).
fn draw_picture(path: &str, w: usize, h: usize) -> Vec<Line> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if meta.len() > MOST_BYTES {
        return Vec::new();
    }
    let Ok(r) = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) else {
        return Vec::new();
    };
    let Ok((pw, ph)) = r.into_dimensions() else {
        return Vec::new();
    };
    if u64::from(pw) * u64::from(ph) > MOST_PIXELS {
        return Vec::new();
    }
    let Ok(src) = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|_| ())
        .and_then(|r| r.decode().map_err(|_| ()))
    else {
        return Vec::new();
    };
    // Blocks take the cells the picture would take sharp: no bigger.
    let (cols, rows) = fit(pw as usize, ph as usize, w, h);
    if cols == 0 {
        return Vec::new();
    }
    let px = src
        .resize_exact((2 * cols) as u32, (4 * rows) as u32, FilterType::CatmullRom)
        .to_rgb8();
    quadrants(&px)
}

/// The image at path drawn to fit w cells by h rows, empty when it can't
/// be read: in half blocks, or when fine in quadrants, twice as many
/// pixels across (Preview). It decodes, so it's for off the UI thread.
pub fn preview(path: &str, w: usize, h: usize, fine: bool) -> Vec<Line> {
    let px = if fine {
        read_thumb(path, 2 * w, 4 * h)
    } else {
        read_thumb(path, w, 2 * h)
    };
    let Some(px) = px else {
        return Vec::new();
    };
    if fine {
        // Transparency shows as black, as Go's.
        let rgb = image::RgbImage::from_fn(px.width(), px.height(), |x, y| {
            let p = px.get_pixel(x, y).0;
            image::Rgb([p[0], p[1], p[2]])
        });
        return quadrants(&rgb);
    }
    half_blocks(&px)
}

/// The image at path shrunk to fit w by h pixels; None when it can't be
/// read or is too big to decode (readThumb).
fn read_thumb(path: &str, w: usize, h: usize) -> Option<image::RgbaImage> {
    let r = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .ok()?;
    let (pw, ph) = r.into_dimensions().ok()?;
    if u64::from(pw) * u64::from(ph) > MOST_PIXELS {
        return None;
    }
    let src = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    shrink_to(&src, w, h)
}

/// src scaled to fit max_w by max_h, keeping its shape, each pixel the
/// average of those it covers, summed premultiplied as Go's colours are
/// (shrinkTo).
fn shrink_to(src: &image::RgbaImage, max_w: usize, max_h: usize) -> Option<image::RgbaImage> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    if sw == 0 || sh == 0 {
        return None;
    }
    let mut w = sw.min(max_w);
    let mut h = ((sh * w + sw / 2) / sw.max(1)).max(1);
    if h > max_h {
        h = max_h;
        w = ((sw * h + sh / 2) / sh).max(1);
    }
    let mut out = image::RgbaImage::new(w as u32, h as u32);
    for y in 0..h {
        let (y0, y1) = (y * sh / h, (y + 1) * sh / h);
        for x in 0..w {
            let (x0, x1) = (x * sw / w, (x + 1) * sw / w);
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1.max(y0 + 1) {
                for sx in x0..x1.max(x0 + 1) {
                    let p = src.get_pixel(sx as u32, sy as u32).0;
                    // color.NRGBA's RGBA: 16 bits, premultiplied.
                    let ca = u64::from(p[3]) * 0x101;
                    let pre = |c: u8| u64::from(c) * 0x101 * ca / 0xffff;
                    (r, g, b, a, n) = (r + pre(p[0]), g + pre(p[1]), b + pre(p[2]), a + ca, n + 1);
                }
            }
            if a == 0 {
                continue;
            }
            out.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    (r * 255 / a) as u8,
                    (g * 255 / a) as u8,
                    (b * 255 / a) as u8,
                    (a / n / 257) as u8,
                ]),
            );
        }
    }
    Some(out)
}

/// px two rows of pixels to a line: each cell's ▀ takes the upper as its
/// colour and the lower as its ground; a pixel less than half opaque
/// shows the terminal's ground (halfBlocks).
pub fn half_blocks(px: &image::RgbaImage) -> Vec<Line> {
    let (w, h) = (px.width(), px.height());
    let rgb = |p: [u8; 4]| Color::Rgb(p[0], p[1], p[2]);
    let sameish = |a: [u8; 4], b: [u8; 4]| {
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(a[0], b[0]) + d(a[1], b[1]) + d(a[2], b[2]) < FLAT
    };
    let mut rows = Vec::new();
    for y in (0..h).step_by(2) {
        let mut l = Line::new();
        for x in 0..w {
            let top = px.get_pixel(x, y).0;
            let bot = if y + 1 < h {
                px.get_pixel(x, y + 1).0
            } else {
                [0; 4]
            };
            let span = match (top[3] >= 128, bot[3] >= 128) {
                (true, true) if sameish(top, bot) => {
                    Span::new(" ", Style::default().on(rgb(top))) // no glyph, so no seam
                }
                (true, true) => Span::new("▀", Style::fg(rgb(top)).on(rgb(bot))),
                (true, false) => Span::new("▀", Style::fg(rgb(top))),
                (false, true) => Span::new("▄", Style::fg(rgb(bot))),
                (false, false) => Span::plain(" "),
            };
            l.spans.push(span);
        }
        rows.push(l);
    }
    rows
}

/// The quadrant blocks by which of a cell's four take the foreground: 1
/// top left, 2 top right, 4 bottom left, 8 bottom right.
const QUAD: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// How far apart, squared across red, green and blue, two colours can be
/// and still be drawn as one: about 20 a channel.
const FLAT: i32 = 3 * 20 * 20;

/// px two pixels across and four down to a cell, the four down taken in
/// pairs so each quarter of the cell is square (quadrants).
pub fn quadrants(px: &image::RgbImage) -> Vec<Line> {
    let (w, h) = (px.width(), px.height());
    let at = |x: u32, y: u32| -> [i32; 3] {
        if x >= w || y >= h {
            return [0; 3];
        }
        let p = px.get_pixel(x, y).0;
        [p[0] as i32, p[1] as i32, p[2] as i32]
    };
    let mut rows = Vec::new();
    for y in (0..h).step_by(4) {
        let mut l = Line::new();
        for x in (0..w).step_by(2) {
            let mut q = [[0i32; 3]; 4];
            for (i, c) in q.iter_mut().enumerate() {
                let (dx, dy) = ((i % 2) as u32, (i / 2 * 2) as u32);
                let (top, bot) = (at(x + dx, y + dy), at(x + dx, y + dy + 1));
                for k in 0..3 {
                    c[k] = (top[k] + bot[k]) / 2;
                }
            }
            l.spans.push(quad_cell(&q));
        }
        rows.push(l);
    }
    rows
}

/// One cell's four quarters in its two colours (quadCell).
fn quad_cell(q: &[[i32; 3]; 4]) -> Span {
    let dist = |a: &[i32; 3], c: &[i32; 3]| -> i32 {
        (a[0] - c[0]).pow(2) + (a[1] - c[1]).pow(2) + (a[2] - c[2]).pow(2)
    };
    let rgb = |c: [i32; 3], n: i32| -> Color {
        let n = n.max(1);
        Color::Rgb((c[0] / n) as u8, (c[1] / n) as u8, (c[2] / n) as u8)
    };
    // The two quarters furthest apart seed the two colours.
    let (mut sa, mut sb, mut far) = (0, 0, -1);
    for i in 0..4 {
        for j in i + 1..4 {
            let d = dist(&q[i], &q[j]);
            if d > far {
                (sa, sb, far) = (i, j, d);
            }
        }
    }
    // A cell near enough one colour is drawn as ground alone: a glyph's
    // edge leaves a seam, and a page's flat white would be lined with them.
    if far < FLAT {
        let mut sum = [0; 3];
        for c in q {
            for k in 0..3 {
                sum[k] += c[k];
            }
        }
        return Span::new(
            " ",
            Style {
                bg: rgb(sum, 4),
                ..Style::default()
            },
        );
    }
    let (mut mask, mut fg, mut bg, mut nf, mut nb) = (0, [0; 3], [0; 3], 0, 0);
    for (i, c) in q.iter().enumerate() {
        if dist(c, &q[sa]) <= dist(c, &q[sb]) {
            mask |= 1 << i;
            for k in 0..3 {
                fg[k] += c[k];
            }
            nf += 1;
        } else {
            for k in 0..3 {
                bg[k] += c[k];
            }
            nb += 1;
        }
    }
    Span::new(
        QUAD[mask].to_string(),
        Style {
            fg: rgb(fg, nf),
            bg: rgb(bg, nb),
            ..Style::default()
        },
    )
}

/// Pictures side by side, gap cells apart, wrapping to w cells; each row
/// of them top-aligned, a blank row between. Where each landed: x, y,
/// width and height (Tile).
pub fn tile(pics: &[Vec<Line>], w: usize, gap: usize) -> (Vec<Line>, Vec<[usize; 4]>) {
    let pw = |p: &Vec<Line>| p.first().map_or(0, Line::width);
    let mut rows: Vec<Line> = Vec::new();
    let mut at = vec![[0usize; 4]; pics.len()];
    let mut group: Vec<usize> = Vec::new();
    let mut x = 0;
    let flush = |rows: &mut Vec<Line>, group: &mut Vec<usize>| {
        if !rows.is_empty() {
            rows.push(Line::new()); // a row of them apart from the last
        }
        let h = group.iter().map(|&i| pics[i].len()).max().unwrap_or(0);
        for y in 0..h {
            let mut l = Line::new();
            for (k, &i) in group.iter().enumerate() {
                if k > 0 {
                    l.spans.push(Span::plain(" ".repeat(gap)));
                }
                match pics[i].get(y) {
                    Some(r) => l.spans.extend(r.spans.iter().cloned()),
                    None => l.spans.push(Span::plain(" ".repeat(pw(&pics[i])))),
                }
            }
            rows.push(l);
        }
        group.clear();
    };
    for (i, p) in pics.iter().enumerate() {
        if p.is_empty() {
            continue;
        }
        let n = pw(p);
        if !group.is_empty() && x + gap + n > w {
            flush(&mut rows, &mut group);
            x = 0;
        }
        if !group.is_empty() {
            x += gap;
        }
        let mut y = rows.len();
        if y > 0 {
            y += 1; // the blank row flush puts first
        }
        at[i] = [x, y, n, p.len()];
        group.push(i);
        x += n;
    }
    if !group.is_empty() {
        flush(&mut rows, &mut group);
    }
    (rows, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(ls: &[Line]) -> Vec<usize> {
        ls.iter().map(Line::width).collect()
    }

    #[test]
    fn fit_keeps_the_shape() {
        // termimg's own cases: a wide picture keeps its width, a tall one
        // its height, and nothing comes out as no cells.
        assert_eq!(fit(1000, 500, 40, 40), (40, 10));
        assert_eq!(fit(500, 1000, 80, 10), (10, 10));
        assert_eq!(fit(4, 4, 40, 40), (1, 1));
        assert_eq!(fit(0, 4, 40, 40), (0, 0));
        assert_eq!(fit(100_000, 10, 1000, 1000), (297, 1));
    }

    // thumb.go halfBlocks: ▀ the upper pixel on the lower, a flat pair a
    // ground alone, a clear pixel the terminal's own ground.
    #[test]
    fn half_blocks_draw_two_pixels_a_cell() {
        let mut px = image::RgbaImage::new(3, 2);
        px.put_pixel(0, 0, image::Rgba([200, 0, 0, 255]));
        px.put_pixel(0, 1, image::Rgba([0, 0, 200, 255]));
        px.put_pixel(1, 0, image::Rgba([10, 10, 10, 255]));
        px.put_pixel(1, 1, image::Rgba([12, 12, 12, 255]));
        px.put_pixel(2, 1, image::Rgba([0, 200, 0, 255]));
        let rows = half_blocks(&px);
        assert_eq!(rows.len(), 1);
        let s = &rows[0].spans;
        assert_eq!(
            (s[0].text.as_str(), s[0].style.fg, s[0].style.bg),
            ("▀", Color::Rgb(200, 0, 0), Color::Rgb(0, 0, 200))
        );
        assert_eq!(
            (s[1].text.as_str(), s[1].style.bg),
            (" ", Color::Rgb(10, 10, 10))
        );
        assert_eq!(
            (s[2].text.as_str(), s[2].style.fg),
            ("▄", Color::Rgb(0, 200, 0))
        );
    }

    // thumb.go shrinkTo: the shape kept, each pixel its block's average.
    #[test]
    fn shrink_averages_and_keeps_the_shape() {
        let mut src = image::RgbaImage::new(4, 2);
        for x in 0..4 {
            for y in 0..2 {
                let c = if x < 2 { 100 } else { 200 };
                src.put_pixel(x, y, image::Rgba([c, c, c, 255]));
            }
        }
        let out = shrink_to(&src, 2, 10).unwrap();
        assert_eq!((out.width(), out.height()), (2, 1));
        assert_eq!(out.get_pixel(0, 0).0, [100, 100, 100, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [200, 200, 200, 255]);
        let tall = shrink_to(&image::RgbaImage::new(2, 40), 10, 4).unwrap();
        assert_eq!((tall.width(), tall.height()), (1, 4));
    }

    #[test]
    fn quadrants_split_a_cell_in_two_colours() {
        // Left half red, right half blue: ▌ red on blue.
        let mut px = image::RgbImage::new(2, 4);
        for y in 0..4 {
            px.put_pixel(0, y, image::Rgb([200, 0, 0]));
            px.put_pixel(1, y, image::Rgb([0, 0, 200]));
        }
        let rows = quadrants(&px);
        assert_eq!(rows.len(), 1);
        let s = &rows[0].spans[0];
        assert_eq!(s.text, "▌");
        assert_eq!(s.style.fg, Color::Rgb(200, 0, 0));
        assert_eq!(s.style.bg, Color::Rgb(0, 0, 200));
        // Near one colour: ground alone.
        let flat = image::RgbImage::from_pixel(2, 4, image::Rgb([250, 250, 250]));
        let s = &quadrants(&flat)[0].spans[0];
        assert_eq!(
            (s.text.as_str(), s.style.bg),
            (" ", Color::Rgb(250, 250, 250))
        );
    }

    #[test]
    fn tile_wraps_and_says_where() {
        let pic = |w: usize, h: usize| vec![Line::from_spans(vec![Span::plain("x".repeat(w))]); h];
        let (rows, at) = tile(&[pic(4, 2), pic(4, 3), pic(4, 1)], 10, 2);
        assert_eq!(at, vec![[0, 0, 4, 2], [6, 0, 4, 3], [0, 4, 4, 1]]);
        assert_eq!(widths(&rows), vec![10, 10, 10, 0, 4]);
    }

    #[test]
    fn a_picture_made_and_one_unreadable() {
        let dir = std::env::temp_dir().join(format!("rush-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbImage::from_pixel(100, 50, image::Rgb([10, 200, 30]))
            .save(&p)
            .unwrap();
        let m = Model::new("0.1.0");
        let rows = picture(&m, p.to_str().unwrap(), 20, 20).expect("made at once with no loop");
        assert_eq!(rows.len(), 3, "100×50 px in 10×20 cells: 10 by 3");
        assert!(rows.iter().all(|l| l.width() == 10));
        assert!(
            picture(&m, "/nonexistent/x.png", 20, 20)
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
