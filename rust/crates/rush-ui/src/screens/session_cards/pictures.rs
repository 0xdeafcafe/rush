//! Pictures in coloured quarter blocks: an image fitted to a box of cells,
//! its shape kept by the cell's size, each made once off the UI thread
//! and read without decoding after. After Go's convo/picture.go (Picture
//! :51, PictureDrawn :58, drawPicture :124, Tile :222), convo/thumb.go
//! (quadGlyphs :102, flatDist :106, quadrants :118, quadCell :150,
//! thumbSlots :54) and photon's termimg (Cell, Fit).
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
