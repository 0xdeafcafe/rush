//! Pieces other screens draw with: the Overview's own sections past a
//! session's report (overviewsections.go), how the stack shares its rows
//! between the main agent and its subagents (stack.go:30-78 and split
//! :128), the shelf of setups on the box's top border (shelf.go), a
//! dictation's look as a box draws it (yap.go:130-215) and the stash's
//! words (stash.go:94-150). Each is a function of what it's given, for
//! the screen that owns the box or the view to call.

use rush_core::adapter::Registry;
use rush_core::text::{Color, Line};

use super::ink::spread;
use crate::style::*;

// --- the Overview's sections ---

/// A row of the Overview: its line, and the ref a click or enter acts on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefLine {
    pub line: Line,
    pub r#ref: String,
}

impl RefLine {
    fn of(line: Line) -> RefLine {
        RefLine {
            line,
            r#ref: String::new(),
        }
    }
}

/// Disk use in a few characters: 1.2G, 340M, 12K, – for none (disk).
pub fn disk(n: i64) -> String {
    if n >= 1 << 30 {
        format!("{:.1}G", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{}M", n >> 20)
    } else if n > 0 {
        format!("{}K", n >> 10)
    } else {
        "–".into()
    }
}

/// One place an agent wrote outside its project and scratch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeftItem {
    pub path: String,
    pub size: i64,
    /// Scratch: under a temp folder.
    pub temp: bool,
    /// What holds it back from being cleaned, if anything.
    pub why: String,
}

/// What holds a left item back, or that it's scratch (leftNote).
fn left_note(l: &LeftItem) -> String {
    if !l.why.is_empty() {
        format!(" · {}", l.why)
    } else if l.temp {
        " · scratch".into()
    } else {
        " · outside temp".into()
    }
}

/// What the agent wrote outside its project and scratch, as last looked
/// at: the places, a few, and what they come to (leftLines). left is how
/// many places, mine those still there.
pub fn left_lines(left: usize, mine: &[LeftItem], w: usize) -> Vec<RefLine> {
    if left == 0 {
        return Vec::new();
    }
    let size: i64 = mine.iter().map(|l| l.size).sum();
    let head = if mine.is_empty() {
        format!("    {left} places written outside it")
    } else {
        format!(
            "    {left} places written outside it, {} still there · {}",
            mine.len(),
            disk(size)
        )
    };
    let mut out = vec![RefLine::of(fit(
        line([sp(format!("{head} · Projects, c cleans up"), DIM)]),
        w,
    ))];
    for (i, l) in mine.iter().enumerate() {
        if i == 5 {
            out.push(RefLine::of(line([sp(
                format!("      and {} more", mine.len() - 5),
                DIM,
            )])));
            break;
        }
        out.push(RefLine::of(fit(
            line([
                plain("      "),
                sp(tildify(&l.path), SUB),
                sp(format!(" · {}{}", disk(l.size), left_note(l)), DIM),
            ]),
            w,
        )));
    }
    out
}

/// What the Overview's own sections are drawn from.
#[derive(Clone, Debug, Default)]
pub struct Overview {
    /// Its pull requests' rows, drawn by the screen that has them.
    pub prs: Vec<RefLine>,
    /// How many artifacts it published, and their rows past their own
    /// heading.
    pub artifacts: usize,
    pub artifact_rows: Vec<RefLine>,
    /// Its scratch folder (TMPDIR), and what's in it.
    pub temp_dir: String,
    pub temp: i64,
    /// The places it wrote outside its project, as left_lines draws them.
    pub left: Vec<RefLine>,
    /// Its memory files and what they add to every session, when it has a
    /// Memory tab.
    pub memory: Option<(usize, i64)>,
    /// The row picked, by ref, and whether the view has the keys.
    pub sel: String,
    pub focused: bool,
}

/// The Overview's own rows past the session's report: its pull requests,
/// the pages it published, its scratch, and a link to the Memory tab
/// (overviewSections).
pub fn overview_sections(o: &Overview, w: usize) -> Vec<RefLine> {
    let mut out = Vec::new();
    let head = |out: &mut Vec<RefLine>, title: &str, meta: &str| {
        let mut h = line([plain("  "), sp("▾", FAINT), plain(" "), spb(title, SUB)]);
        if !meta.is_empty() {
            h.spans.push(plain("  "));
            h.spans.push(sp(meta, DIM));
        }
        let n = w.saturating_sub(h.width() + 3);
        h.spans.push(plain(" "));
        h.spans.push(sp("─".repeat(n), FAINT));
        out.push(RefLine::default());
        out.push(RefLine::of(fit(h, w)));
    };
    out.extend(o.prs.iter().cloned());
    if o.artifacts > 0 {
        head(
            &mut out,
            "Artifacts",
            &format!("{} published · enter opens one", o.artifacts),
        );
        out.extend(o.artifact_rows.iter().cloned());
    }
    if !o.temp_dir.is_empty() {
        // Its scratch, to find what it left behind.
        head(&mut out, "Scratch", "TMPDIR for it and everything it runs");
        let mut row = line([plain("    "), sp(tildify(&o.temp_dir), TEXT)]);
        if o.temp > 0 {
            row.spans.push(sp(format!(" · {}", disk(o.temp)), DIM));
        }
        out.push(RefLine::of(fit(row, w)));
        out.extend(o.left.iter().cloned());
    }
    if let Some((files, up)) = o.memory {
        // A link to the Memory tab, with what it costs.
        head(&mut out, "Memory", "");
        let row = line([
            plain("    "),
            sp("→ ", BLUE),
            sp(format!("{files} files"), TEXT),
            sp(
                format!(" · ≈{} tokens every session", rush_efficiency::tokens(up)),
                DIM,
            ),
        ]);
        let right = line([sp("enter opens the Memory tab", DIM), plain("  ")]);
        let body = fit(spread(row, right, w), w);
        let l = if o.sel == "go:memory" {
            let bar = if o.focused {
                sp("▍", ORANGE)
            } else {
                sp("▍", FAINT)
            };
            let mut l = line([bar]);
            join(&mut l, cells_from(&body, 1));
            highlight(l, w)
        } else {
            body
        };
        out.push(RefLine {
            line: l,
            r#ref: "go:memory".into(),
        });
    }
    out
}

/// l without its first n cells.
fn cells_from(l: &Line, n: usize) -> Line {
    crate::frame::dialog::cells(l, n, l.width())
}

// --- the stack ---

/// How many subagents get a band at most.
pub const STACK_MAX: usize = 4;
/// The stack grows over this many frames from where the panel was.
pub const STACK_FRAMES: usize = 6;

/// Shares rows between n bands, the first ones a row more when it doesn't
/// divide (split).
pub fn split(rows: usize, n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|i| rows / n + usize::from(i < rows % n))
        .collect()
}

/// The stack's rows, h tall, between the main agent and live subagents:
/// each band's height, main first, and how many more live ones get their
/// title row alone (stackLines' sharing).
pub fn stack_heights(h: usize, live: usize) -> (Vec<usize>, usize) {
    let mut avail = h;
    // At most STACK_MAX live bands; the rest of the live ones are their
    // title row alone.
    let n = (live + 1).min(STACK_MAX + 1).min((avail / 4).max(1));
    let rest = live - (n - 1);
    // As many title rows as leave each band its title and a row.
    let rest = rest.min(avail.saturating_sub(2 * n));
    avail -= rest;
    // The main agent on top, a bigger share; the live ones evenly under.
    let mut heights = vec![avail];
    if n > 1 {
        let main = (avail / n).max(avail * 2 / 5);
        heights = vec![main];
        heights.extend(split(avail - main, n - 1));
    }
    (heights, rest)
}

/// The height a stack growing in from `from` rows has at frame grow of
/// STACK_FRAMES.
pub fn stack_grow(h: usize, from: usize, grow: usize) -> usize {
    if grow >= STACK_FRAMES {
        return h;
    }
    let from = h.min(from.max(2));
    from + (h - from) * grow / STACK_FRAMES
}

// --- the shelf ---

/// How many setups the shelf holds: one per ⌥ digit.
pub const SHELF_MAX: usize = 9;

/// A setup as the shelf names it: whose model, in which harness when not
/// its own, which model (shelfWord).
pub fn shelf_word(reg: &Registry, kind: &str, key_billed: bool, model: &str) -> String {
    let mut word = reg.provider_label(&reg.provider_of(kind));
    let h = reg.harness_of(kind);
    if h.as_str() != kind {
        word = format!("{word}·{}", reg.harness_label(h.as_str()).to_lowercase());
    }
    if key_billed {
        word.push_str("·key");
    }
    if !model.is_empty() {
        word = format!("{word}·{}", reg.model_name(kind, model));
    }
    word
}

/// The shelf's ⌥1…9: which tile, from 0 (shelfKey).
pub fn shelf_key(s: &str) -> Option<usize> {
    let d = s.strip_prefix("alt+")?;
    let b = d.as_bytes();
    (b.len() == 1 && (b'1'..=b'9').contains(&b[0])).then(|| (b[0] - b'1') as usize)
}

/// Where a tile landed on the shelf's line, by cells from its left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShelfHit {
    pub x0: usize,
    pub x1: usize,
    /// The tile's index in what was given.
    pub tile: usize,
}

/// The shelf as the box's border shows it, at most w wide: each tile's
/// text, the chosen one as its chip; too wide, the others go from the
/// end and the chosen one stays (shelfLine). tiles are (text, chosen).
pub fn shelf_line(tiles: &[(Line, bool)], w: usize) -> (Line, Vec<ShelfHit>) {
    let mut segs: Vec<usize> = (0..tiles.len()).collect();
    let width = |segs: &[usize]| {
        segs.iter()
            .map(|&i| tiles[i].0.width() + 1)
            .sum::<usize>()
            .saturating_sub(1)
    };
    while segs.len() > 1 && width(&segs) > w {
        let mut at = segs.len() - 1;
        if tiles[segs[at]].1 {
            at -= 1;
        }
        segs.remove(at);
    }
    let mut l = Line::new();
    let mut hits = Vec::new();
    for (n, &i) in segs.iter().enumerate() {
        if n > 0 {
            l.spans.push(plain(" "));
        }
        let x0 = l.width();
        join(&mut l, tiles[i].0.clone());
        hits.push(ShelfHit {
            x0,
            x1: l.width(),
            tile: i,
        });
    }
    (l, hits)
}

/// A tile as the shelf shows it when it isn't the chosen one: its ⌥
/// number, then its word.
pub fn shelf_tile(n: usize, word: &str) -> Line {
    line([sp(format!("⌥{n}"), DIM), plain(" "), sp(word, TEXT)])
}

// --- dictation's look ---

/// The look's colours, in steps from each's resting colour to its lit one.
pub const YAP_STEPS: usize = 16;

/// A dictation as a box draws it (liveText).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveText {
    pub settled: String,
    pub guessing: String,
    /// How loud, 0 to 1.
    pub level: f64,
    pub frame: usize,
    /// What's going on, in the box's bottom edge.
    pub hint: String,
}

/// What yap says of a dictation (yap.State, as its plugin writes it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct YapState {
    /// hidden, listening, finishing, done.
    pub phase: String,
    pub settled: String,
    pub guessing: String,
    /// 0 to 100.
    pub level: i64,
    pub locked: bool,
    /// The app the dictation goes to.
    pub app: String,
}

impl YapState {
    pub fn active(&self) -> bool {
        matches!(self.phase.as_str(), "listening" | "finishing" | "done")
    }

    /// The dictation as a box draws it (yapLook).
    pub fn look(&self, frame: usize) -> LiveText {
        let hint = if self.phase != "listening" {
            "pasting"
        } else if self.locked {
            "listening · tap fn to paste"
        } else {
            "listening · let go to paste"
        };
        LiveText {
            settled: self.settled.clone(),
            guessing: if self.phase == "done" {
                String::new()
            } else {
                self.guessing.clone()
            },
            level: self.level as f64 / 100.0,
            frame,
            hint: hint.into(),
        }
    }
}

/// The step of the look's colours k (0 to 1) is (yapStep).
pub fn yap_step(k: f64) -> usize {
    (k.clamp(0.0, 1.0) * YAP_STEPS as f64 + 0.5) as usize
}

/// How lit the text at column x of row y is: a band of light leaning
/// like yap's, crossing w columns every 1.6s, as wide as a third of them
/// (yapSweep).
pub fn yap_sweep(frame: usize, x: usize, y: usize, w: usize) -> f64 {
    const PERIOD: usize = 32; // frames a crossing
    let band = (w as f64 / 3.0).clamp(3.0, 8.0);
    let at = -band + (frame % PERIOD) as f64 / PERIOD as f64 * (w as f64 + 2.0 * band);
    let d = (x as f64 + 2.0 * y as f64 - at).abs();
    if d >= band {
        return 0.0;
    }
    let k = 1.0 - d / band;
    k * k
}

/// How lit a cell of the box's edge is, `around` cells clockwise from its
/// top left out of n: a light with a fading tail, round the box every
/// 2.4s (yapGlint).
pub fn yap_glint(frame: usize, around: usize, n: usize) -> f64 {
    const LAP: usize = 48;
    const TAIL: f64 = 28.0;
    if n == 0 {
        return 0.0;
    }
    let at = (frame % LAP) * n / LAP;
    let d = ((at as isize - around as isize) % n as isize + n as isize) % n as isize;
    let d = d as f64;
    if d >= TAIL {
        return 0.0;
    }
    let k = 1.0 - d / TAIL;
    k * k
}

/// The look's inks at step i of YAP_STEPS: the settled words, the guessed
/// ones, the box's edge, and the cursor's ground (yapColours).
pub fn yap_ink(i: usize) -> (Color, Color, Color, Color) {
    let t = i.min(YAP_STEPS) as f64 / YAP_STEPS as f64;
    let text = Color::Rgb(226, 221, 211);
    let lit = Color::Rgb(255, 250, 240);
    let ground = Color::Rgb(40, 36, 32);
    let (orange, glint) = (Color::Rgb(217, 119, 87), Color::Rgb(255, 232, 205));
    (
        mix(text, lit, t),
        mix(mix(text, ground, 0.5), lit, t),
        mix(orange, glint, t),
        mix(mix(orange, ground, 0.55), mix(orange, glint, 0.25), t),
    )
}

/// How long after yap says it's done its paste is taken as the
/// dictation's, in ms.
pub const YAP_PASTE_WAIT: i64 = 5_000;

/// Whether a paste is the dictation yap just finished, which goes in as
/// typed (yapPaste): final, said at final_at.
pub fn is_dictation(final_: &str, final_at: i64, now: i64, text: &str) -> bool {
    !final_.is_empty() && now - final_at < YAP_PASTE_WAIT && text.trim() == final_.trim()
}

/// Whether this window shows the dictation: one is under way, rush's
/// terminal has the keys, and it's going to rush's terminal (yapShown).
pub fn yap_shown(s: &YapState, focused: bool, this_app: &str) -> bool {
    s.active() && focused && (s.app.is_empty() || this_app.is_empty() || s.app == this_app)
}

// --- the stash's words ---

/// What the stash is called, as its settings say (drafts.Words).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StashWords {
    /// The history's: Stash.
    pub title: &'static str,
    /// One set aside: your stash.
    pub noun: &'static str,
}

impl StashWords {
    /// The # command rush opens the history with: #stash.
    pub fn command(&self) -> String {
        format!("#{}", self.title.to_lowercase())
    }
}

/// The keys the stash names, filled in as you have them: aside the key
/// that sets the box aside, history the one that opens what was kept
/// (stashSaid).
pub fn stash_said(s: &str, aside: &str, history: &str) -> String {
    s.replace("{aside}", aside).replace("{history}", history)
}

/// What a flash says of text taken out of a box: what happened, and the
/// history's tab that has it, when the stash is running to keep it
/// (wentTo).
pub fn went_to(did: &str, tab: &str, on: bool, history: &str) -> String {
    if !on {
        return did.to_string();
    }
    format!("{did} · {history} › {tab} has it")
}

/// The undo keys' words, for flashes (undoHint).
pub const UNDO_HINT: &str = "cmd+z or ctrl+/";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disks() {
        assert_eq!(disk(0), "–");
        assert_eq!(disk(2048), "2K");
        assert_eq!(disk(5 << 20), "5M");
        assert_eq!(disk(3 << 30), "3.0G");
    }

    #[test]
    fn sections() {
        let o = Overview {
            temp_dir: "/tmp/x".into(),
            temp: 3 << 20,
            left: left_lines(
                2,
                &[LeftItem {
                    path: "/var/out".into(),
                    size: 4096,
                    ..LeftItem::default()
                }],
                80,
            ),
            memory: Some((3, 12_500)),
            sel: "go:memory".into(),
            focused: true,
            ..Overview::default()
        };
        let rows = overview_sections(&o, 80);
        let text: Vec<String> = rows.iter().map(|r| r.line.plain()).collect();
        assert!(
            text[1].starts_with("  ▾ Scratch  TMPDIR for it and everything it runs ─"),
            "{text:?}"
        );
        assert_eq!(text[2].trim_end(), "    /tmp/x · 3M");
        assert!(
            text[3].contains(
                "2 places written outside it, 1 still there · 4K · Projects, c cleans up"
            )
        );
        assert!(text[4].contains("/var/out · 4K · outside temp"));
        let last = rows.last().unwrap();
        assert_eq!(last.r#ref, "go:memory");
        assert!(
            last.line
                .plain()
                .starts_with("▍   → 3 files · ≈12k tokens every session"),
            "{:?}",
            last.line.plain()
        );
        assert!(last.line.plain().ends_with("enter opens the Memory tab  "));
        assert!(rows.iter().all(|r| r.line.width() <= 80));
    }

    #[test]
    fn stack_sharing() {
        assert_eq!(split(10, 3), vec![4, 3, 3]);
        assert_eq!(split(9, 3), vec![3, 3, 3]);
        let (h, rest) = stack_heights(40, 3);
        assert_eq!(h.len(), 4);
        assert_eq!(h.iter().sum::<usize>(), 40);
        assert!(h[0] > h[1] && h[1] - h[3] <= 1, "{h:?}");
        assert_eq!(rest, 0);
        let (h, rest) = stack_heights(40, 9);
        assert_eq!((h.len(), rest), (STACK_MAX + 1, 5));
        assert_eq!(stack_grow(40, 10, STACK_FRAMES), 40);
        assert_eq!(stack_grow(40, 10, 3), 25);
    }

    #[test]
    fn shelves() {
        assert_eq!(shelf_key("alt+1"), Some(0));
        assert_eq!(shelf_key("alt+9"), Some(8));
        assert_eq!(shelf_key("alt+x"), None);
        assert_eq!(shelf_key("alt+0"), None);
        let tiles = vec![
            (line([sp("chip", TEXT)]), true),
            (shelf_tile(2, "openai"), false),
            (shelf_tile(3, "kimi"), false),
        ];
        let (l, hits) = shelf_line(&tiles, 80);
        assert_eq!(l.plain(), "chip ⌥2 openai ⌥3 kimi");
        assert_eq!(
            hits[1],
            ShelfHit {
                x0: 5,
                x1: 14,
                tile: 1
            }
        );
        let (l, hits) = shelf_line(&tiles, 12);
        assert_eq!(l.plain(), "chip");
        assert_eq!(hits.len(), 1, "the others go, the chosen one stays");
    }

    #[test]
    fn dictation_look() {
        let s = YapState {
            phase: "listening".into(),
            settled: "the deprecation".into(),
            guessing: "of the old servic".into(),
            level: 50,
            ..YapState::default()
        };
        let l = s.look(3);
        assert_eq!(
            (l.hint.as_str(), l.level),
            ("listening · let go to paste", 0.5)
        );
        assert!(yap_shown(&s, true, "com.mitchellh.ghostty"));
        let notes = YapState {
            app: "com.apple.Notes".into(),
            ..s.clone()
        };
        assert!(!yap_shown(&notes, true, "com.mitchellh.ghostty"));
        assert!(
            !yap_shown(&s, false, ""),
            "not while the terminal hasn't the keys"
        );
        // The edge's light moves round the box.
        let lit = |f| (0..40).map(|a| yap_glint(f, a, 40)).fold(0.0, f64::max);
        assert!(lit(0) > 0.9 && lit(12) > 0.9);
        assert_ne!(
            (0..40).position(|a| yap_glint(0, a, 40) == 1.0),
            (0..40).position(|a| yap_glint(12, a, 40) == 1.0)
        );
        assert!(yap_sweep(16, 20, 0, 40) > 0.0 && yap_sweep(0, 39, 0, 40) == 0.0);
        assert_eq!(
            (yap_step(0.0), yap_step(1.0), yap_step(2.0)),
            (0, YAP_STEPS, YAP_STEPS)
        );
        assert_eq!(yap_ink(0).0, Color::Rgb(226, 221, 211));
        assert!(is_dictation("ship it", 1_000, 2_000, "ship it "));
        assert!(!is_dictation("ship it", 1_000, 7_000, "ship it"));
    }

    #[test]
    fn stash_words() {
        let w = StashWords {
            title: "Stash",
            noun: "your stash",
        };
        assert_eq!(w.command(), "#stash");
        assert_eq!(
            stash_said(
                "nothing to undo · {history} has what you typed before",
                "ctrl+s",
                "#stash"
            ),
            "nothing to undo · #stash has what you typed before"
        );
        assert_eq!(
            went_to("cleared", "Cleared", true, "#stash"),
            "cleared · #stash › Cleared has it"
        );
        assert_eq!(went_to("cleared", "Cleared", false, "#stash"), "cleared");
    }
}
