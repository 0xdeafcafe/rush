//! Pictures as Kitty graphics, drawn with Unicode placeholders: each cell
//! of a picture is U+10EEEE with its row and column as diacritics, the
//! image's id its colour. A placeholder is a character like any other, so
//! a row holding one is cached, scrolled, clipped and diffed as text is,
//! and the picture follows it; the image itself is sent to the terminal
//! once, the first frame a cell of it is written, and never again while
//! the terminal keeps it. After photon's termimg (Detect :39, Fit :113, ID
//! :131, Placeholders :140, Blank :159, Transmit :182), as Go's
//! convo/picture.go (drawPicture :124) and ui/pictures.go used it, and the
//! protocol (sw.kovidgoyal.net/kitty/graphics-protocol).
//!
//! Whether the terminal takes them is asked as rush takes it (term.rs
//! ask_colours): a graphics query with DA1 after it, which every terminal
//! answers, as the fence; an `OK` for the query before DA1's answer is a
//! yes. Not inside tmux or screen (which pass none of it through), and
//! not when the answer comes late (a slow link): pictures are blocks then.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use ratatui::buffer::Cell;
use ratatui::style::Color as RColor;
use rush_core::text::{Color, Line, Span, Style};

/// The character a picture's cells are drawn with.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The row and column diacritics, by number (kitty's
/// rowcolumn-diacritics.txt): a picture is at most this many cells each way.
const DIACRITICS: [u32; 297] = [
    0x305, 0x30D, 0x30E, 0x310, 0x312, 0x33D, 0x33E, 0x33F, 0x346, 0x34A, 0x34B, 0x34C, 0x350,
    0x351, 0x352, 0x357, 0x35B, 0x363, 0x364, 0x365, 0x366, 0x367, 0x368, 0x369, 0x36A, 0x36B,
    0x36C, 0x36D, 0x36E, 0x36F, 0x483, 0x484, 0x485, 0x486, 0x487, 0x592, 0x593, 0x594, 0x595,
    0x597, 0x598, 0x599, 0x59C, 0x59D, 0x59E, 0x59F, 0x5A0, 0x5A1, 0x5A8, 0x5A9, 0x5AB, 0x5AC,
    0x5AF, 0x5C4, 0x610, 0x611, 0x612, 0x613, 0x614, 0x615, 0x616, 0x617, 0x657, 0x658, 0x659,
    0x65A, 0x65B, 0x65D, 0x65E, 0x6D6, 0x6D7, 0x6D8, 0x6D9, 0x6DA, 0x6DB, 0x6DC, 0x6DF, 0x6E0,
    0x6E1, 0x6E2, 0x6E4, 0x6E7, 0x6E8, 0x6EB, 0x6EC, 0x730, 0x732, 0x733, 0x735, 0x736, 0x73A,
    0x73D, 0x73F, 0x740, 0x741, 0x743, 0x745, 0x747, 0x749, 0x74A, 0x7EB, 0x7EC, 0x7ED, 0x7EE,
    0x7EF, 0x7F0, 0x7F1, 0x7F3, 0x816, 0x817, 0x818, 0x819, 0x81B, 0x81C, 0x81D, 0x81E, 0x81F,
    0x820, 0x821, 0x822, 0x823, 0x825, 0x826, 0x827, 0x829, 0x82A, 0x82B, 0x82C, 0x82D, 0x951,
    0x953, 0x954, 0xF82, 0xF83, 0xF86, 0xF87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17,
    0x1A75, 0x1A76, 0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F,
    0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1,
    0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3,
    0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF,
    0x1DE0, 0x1DE1, 0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5,
    0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0,
    0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC,
    0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8,
    0x2DF9, 0x2DFA, 0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1,
    0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB,
    0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE,
    0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38,
    0x1D185, 0x1D186, 0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242,
    0x1D243, 0x1D244,
];

/// The diacritic for row or column i.
fn diacritic(i: usize) -> char {
    char::from_u32(DIACRITICS[i.min(DIACRITICS.len() - 1)]).unwrap_or('\u{305}')
}

/// The most cells a picture takes each way: the diacritics there are.
pub const MOST_CELLS: usize = DIACRITICS.len();

/// Whether pictures go out as Kitty graphics here, as rush found when it
/// took the terminal; false until then, and off a terminal.
static ON: AtomicBool = AtomicBool::new(false);

/// Whether pictures can be drawn sharp: the terminal said it takes Kitty
/// graphics, and nothing between rush and it would lose them.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub(crate) fn set_on(v: bool) {
    ON.store(v, Ordering::Relaxed);
}

/// The query asked as rush takes the terminal: a 1×1 RGB image, id 31,
/// only checked, never kept (the protocol's own way to ask).
pub(crate) const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// What the terminal said to [`QUERY`] in what it sent back: Some(true)
/// for its `OK`, Some(false) for an error, None when it didn't answer.
pub(crate) fn reply(b: &[u8]) -> Option<bool> {
    let mut rest = b;
    while let Some(i) = rest.windows(3).position(|w| w == b"\x1b_G") {
        let from = &rest[i + 3..];
        let end = from.windows(2).position(|w| w == b"\x1b\\")?;
        let body = String::from_utf8_lossy(&from[..end]);
        let (keys, msg) = body.split_once(';').unwrap_or((&body, ""));
        if keys.split(',').any(|k| k == "i=31") {
            return Some(msg == "OK");
        }
        rest = &from[end + 2..];
    }
    None
}

/// Whether pictures go out as Kitty graphics, from the environment and
/// what the terminal answered: RUSH_GRAPHICS says `kitty` or `blocks`
/// outright (Go's override); inside tmux or screen, never, as they pass
/// none of it through; else only on the terminal's own `OK`.
pub(crate) fn choose(env: impl Fn(&str) -> Option<String>, answered: Option<bool>) -> bool {
    match env("RUSH_GRAPHICS")
        .map(|v| v.to_ascii_lowercase())
        .as_deref()
    {
        Some("kitty") => return true,
        Some("blocks") => return false,
        _ => {}
    }
    worth_asking(env) && answered == Some(true)
}

/// Whether the terminal is asked at all: not through tmux or screen,
/// which swallow the question or answer it for a terminal they then draw
/// over, nor where RUSH_GRAPHICS has said.
pub(crate) fn worth_asking(env: impl Fn(&str) -> Option<String>) -> bool {
    if env("RUSH_GRAPHICS").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let term = env("TERM").unwrap_or_default();
    let muxed = env("TMUX").is_some_and(|v| !v.is_empty())
        || term.starts_with("screen")
        || term.starts_with("tmux");
    !muxed
}

/// A picture's id: from its bytes and the cells it takes, so the same
/// picture is the same id every time and a row drawn with it once stays
/// right. 24 bits, the most a colour holds; never 0 (termimg.ID, FNV-1a).
pub fn id(data: &[u8], cols: usize, rows: usize) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let size = format!("{cols}x{rows}");
    for &b in data.iter().chain(size.as_bytes()) {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0xff_ffff).max(1)
}

/// The colour a picture's cells carry: its id.
fn colour(id: u32) -> Color {
    Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
}

/// The id a cell's colour stands for; 0 for none.
fn id_of(c: Color) -> u32 {
    match c {
        Color::Rgb(r, g, b) => u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
        _ => 0,
    }
}

/// The rows of cols×rows placeholders that show image id
/// (termimg.Placeholders): each cell the placeholder, its row's and
/// column's diacritics, the id its colour.
pub fn placeholders(id: u32, cols: usize, rows: usize) -> Vec<Line> {
    let (cols, rows) = (cols.min(MOST_CELLS), rows.min(MOST_CELLS));
    let st = Style::fg(colour(id));
    (0..rows)
        .map(|r| {
            let mut s = String::with_capacity(cols * 8);
            for c in 0..cols {
                s.push(PLACEHOLDER);
                s.push(diacritic(r));
                s.push(diacritic(c));
            }
            Line::from_spans(vec![Span::new(s, st)])
        })
        .collect()
}

/// Has the terminal show png (a PNG's bytes) as cols×rows cells, giving
/// back the rows of placeholders that show it, to draw anywhere. It's
/// kept, and sent the first frame one of its cells is written.
pub fn picture(id: u32, png: Arc<[u8]>, cols: usize, rows: usize) -> Vec<Line> {
    let (cols, rows) = (cols.clamp(1, MOST_CELLS), rows.clamp(1, MOST_CELLS));
    let mut s = store();
    if !s.images.contains_key(&id) {
        s.bytes += png.len();
        s.order.push_back(id);
        s.images.insert(id, Image { png, cols, rows });
        // The oldest let go past the most kept, and the terminal told to
        // drop its copy; one drawn again after is blank.
        while s.bytes > MOST_BYTES && s.order.len() > 1 {
            let Some(old) = s.order.pop_front() else {
                break;
            };
            if let Some(i) = s.images.remove(&old) {
                s.bytes -= i.png.len();
            }
            if s.sent.remove(&old) {
                s.owed.push(old);
            }
        }
    }
    drop(s);
    placeholders(id, cols, rows)
}

/// The most bytes of pictures kept to send again (after another program
/// had the terminal): the oldest go past it.
const MOST_BYTES: usize = 64 << 20;

struct Image {
    png: Arc<[u8]>,
    cols: usize,
    rows: usize,
}

#[derive(Default)]
struct Store {
    images: HashMap<u32, Image>,
    /// In the order they came, oldest first.
    order: VecDeque<u32>,
    bytes: usize,
    /// Those the terminal has.
    sent: HashSet<u32>,
    /// Those let go that the terminal still holds, to delete.
    owed: Vec<u32>,
}

fn store() -> MutexGuard<'static, Store> {
    static S: LazyLock<Mutex<Store>> = LazyLock::new(Mutex::default);
    S.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether c is the colour of a picture rush has: a placeholder in any
/// other (faded or recoloured since) is drawn blank, as Go blanks a
/// faded screen's pictures (termimg.Blank).
pub(crate) fn known(c: Color) -> bool {
    let id = id_of(c);
    id != 0 && store().images.contains_key(&id)
}

/// Whether a cell's symbol is a picture's.
pub(crate) fn is_placeholder(sym: &str) -> bool {
    sym.starts_with(PLACEHOLDER)
}

/// The terminal no longer has the pictures it was sent (another program
/// had it): each is sent again the next time a cell of it is written.
pub(crate) fn screen_lost() {
    store().sent.clear();
}

/// What's written before a frame's cells: the pictures among them the
/// terminal hasn't been sent, each once, and deletions of those let go.
/// Nothing on a frame whose pictures are on screen already.
pub(crate) fn before_frame<'a>(cells: impl Iterator<Item = &'a Cell>) -> Vec<u8> {
    let mut ids: Vec<u32> = Vec::new();
    for c in cells {
        if !is_placeholder(c.symbol()) {
            continue;
        }
        if let RColor::Rgb(r, g, b) = c.fg {
            let id = id_of(Color::Rgb(r, g, b));
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let mut s = store();
    if ids.is_empty() && s.owed.is_empty() {
        return Vec::new();
    }
    let mut out = String::new();
    for id in std::mem::take(&mut s.owed) {
        out.push_str(&delete(id));
    }
    for id in ids {
        if s.sent.contains(&id) {
            continue;
        }
        if let Some(i) = s.images.get(&id) {
            out.push_str(&transmit(id, &i.png, i.cols, i.rows));
            s.sent.insert(id);
        }
    }
    out.into_bytes()
}

/// The most base64 a chunk carries (the protocol's 4096).
const CHUNK: usize = 4096;

/// The escapes that send png to the terminal as image id with a virtual
/// placement cols×rows cells, for placeholders to show: PNG, in the
/// escapes themselves, in chunks, quiet (termimg.Transmit).
pub(crate) fn transmit(id: u32, png: &[u8], cols: usize, rows: usize) -> String {
    let data = crate::term::base64(png);
    let chunks: Vec<&str> = if data.is_empty() {
        vec![""]
    } else {
        // base64 is ASCII: any cut is on a char's edge.
        (0..data.len())
            .step_by(CHUNK)
            .map(|i| &data[i..(i + CHUNK).min(data.len())])
            .collect()
    };
    let mut out = String::with_capacity(data.len() + chunks.len() * 16 + 64);
    for (i, c) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Ga=T,U=1,f=100,t=d,i={id},c={cols},r={rows},C=1,q=2,m={more};{c}\x1b\\"
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={more},q=2;{c}\x1b\\"));
        }
    }
    out
}

/// The escape that deletes image id and frees its data.
pub(crate) fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

#[cfg(test)]
mod tests;
