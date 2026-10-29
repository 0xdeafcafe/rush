//! Settings › Keys: every action rush's keys do and the keys it has, one
//! place at a time (1-9 or ← → pick it). enter takes the keys you press
//! next (a chord is several, ended with enter), a adds another, x takes
//! them all away, r puts rush's back. What you change goes in
//! keybindings.json. After Go's internal/ui/settings_keys.go (keyPlaces
//! :31, keyContext :47, keyRows :57, showKey :69, contextWhere :84,
//! keyCaps :107, keysControls :120, keysBody :142, keysBoard :233,
//! keysModal :256, keysKey :292, takeKey :325, saveBinding :365,
//! practiceKey :389, practiced :428, practicedPrefix :446, keysHover
//! :461) and keybind.go:31-66 (keyActions, setKeys, keyMap).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rush_core::text::{Line, Span, width};
use rush_keymap::{Action, CONTEXTS, Context, File, Map, Seq};

use super::keyboard::{KB_WIDE, key_parts, keyboard, keycap, legend};
use super::workspace::Geometry;
use super::{State, dialog, dialog_mut};
use crate::frame::confirm::Confirm;
use crate::frame::sheet::window;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// How long a chord waits for its next key (chordWait).
pub const CHORD_WAIT: Duration = Duration::from_secs(2);

/// The keymap as Keys sees and changes it: keybindings.json as read, the
/// map built from it, and keys being taken for an action (keysPage).
#[derive(Default)]
pub struct Keys {
    pub file: File,
    map: Option<Map>,
    /// Where keybindings.json is; rush's own when None (tests set one).
    pub path: Option<PathBuf>,
    /// keybindings.json has been asked for.
    loading: bool,
    /// The action keys are being taken for; empty when none are.
    pub taking: String,
    /// Added to its keys, rather than replacing them.
    pub adding: bool,
    /// The keys pressed so far.
    pub pressed: Vec<String>,
}

/// Keys' own place in the dialog: the context showing, what's being
/// practiced, and the row under the pointer.
#[derive(Clone, Default)]
pub struct KeysDialog {
    /// Which of the contexts with actions Keys shows (keyCtx).
    pub ctx: usize,
    /// A key sequence pressed to try out (practice), and when the last of
    /// it came.
    pub practice: Vec<String>,
    pub practice_at: Option<Instant>,
    /// The row under the pointer, plus one (keyHover).
    pub hover: usize,
}

/// Every action a key can be bound to: rush's keys and its # commands
/// (keyActions; plugins' actions come with the plugins).
fn actions(m: &Model) -> Vec<Action> {
    let mut out = rush_keymap::defaults();
    for c in &m.ext.commands {
        let mut a = Action::new(rush_keymap::command_id(c.name), Context::Any);
        a.title = format!("#{}: {}", c.name, c.about);
        out.push(a);
    }
    out
}

/// The keymap in force (keyMap); rush's own before keybindings.json is
/// read.
pub fn key_map(m: &Model) -> std::borrow::Cow<'_, Map> {
    match m.kept.peek::<State>().and_then(|s| s.keys.map.as_ref()) {
        Some(k) => std::borrow::Cow::Borrowed(k),
        None => {
            std::borrow::Cow::Owned(Map::build(&actions(m), &File::default(), &BTreeMap::new()))
        }
    }
}

/// Puts f in force (setKeys).
pub fn set_keys(m: &mut Model, f: File) {
    let map = Map::build(&actions(m), &f, &BTreeMap::new());
    let k = &mut m.kept::<State>().keys;
    k.file = f;
    k.map = Some(map);
}

/// Reads keybindings.json off the UI thread, once (loadKeys).
pub fn load(m: &mut Model) {
    let k = &mut m.kept::<State>().keys;
    if k.loading || k.map.is_some() {
        return;
    }
    k.loading = true;
    let path = k.path.clone().unwrap_or_else(rush_keymap::path);
    m.later(
        move || rush_keymap::load_from(&path).map_err(|e| e.to_string()),
        |m, r| {
            let f = match r {
                Ok(f) => f,
                Err(e) => {
                    m.flash_err(&format!("keybindings.json: {e}"));
                    File::default()
                }
            };
            set_keys(m, f);
            Out::Draw
        },
    );
}

/// The contexts with actions, in the order the strip shows them
/// (keyPlaces).
pub fn places(km: &Map) -> Vec<Context> {
    let acts = km.actions();
    CONTEXTS
        .iter()
        .copied()
        .filter(|c| acts.iter().any(|a| a.context == *c))
        .collect()
}

/// The context Keys shows (keyContext).
fn context(km: &Map, d: &KeysDialog) -> Option<Context> {
    let cs = places(km);
    cs.get(d.ctx.min(cs.len().saturating_sub(1))).copied()
}

/// The actions of the context Keys shows (keyRows).
pub fn rows(km: &Map, d: &KeysDialog) -> Vec<Action> {
    let Some(c) = context(km, d) else {
        return Vec::new();
    };
    km.actions()
        .into_iter()
        .filter(|a| a.context == c)
        .cloned()
        .collect()
}

/// How many rows Keys has (keysLen).
pub fn len(m: &Model) -> usize {
    rows(&key_map(m), &dialog(m).keys).len()
}

/// Puts Keys on the action id: its context, and the cursor on it
/// (showKey).
pub fn show_key(m: &mut Model, id: &str) {
    let km = key_map(m).into_owned();
    let Some(a) = km.action(id) else {
        return;
    };
    let ctx = places(&km)
        .iter()
        .position(|c| *c == a.context)
        .unwrap_or(0);
    let d = dialog_mut(m);
    d.keys.ctx = ctx;
    if let Some(i) = rows(&km, &d.keys).iter().position(|r| r.id == id) {
        d.cursor = i;
    }
}

/// Where a context's keys work, for the page to say (contextWhere).
pub fn context_where(c: Context) -> &'static str {
    match c {
        Context::Global => "anywhere in rush, over sheets and the command bar too",
        Context::List => "in the Agents list and the Prompt under it",
        Context::Session => "in an agent's Session: its conversation and message box",
        Context::Prompt => "in the Agents prompt while a draft is typed",
        Context::Pages => "on Settings, Projects and Wall pages",
        Context::Any => "in the Agents list or a Session",
    }
}

/// An action's keys as keycaps, a chord in one cap, the alternatives side
/// by side; lit, when it matches one, lights it (keyCaps).
pub fn key_caps(seqs: &[Seq], lit: &str) -> Line {
    if seqs.is_empty() {
        return line([sp("Unbound", TEXT)]);
    }
    let mut l = Line::new();
    for (i, s) in seqs.iter().enumerate() {
        if i > 0 {
            l.spans.push(sp(" or ", TEXT));
        }
        let s = s.to_string();
        let mut cap = keycap(&s, !lit.is_empty() && s == lit);
        if cap.style.fg == SUB {
            cap.style.fg = TEXT;
        }
        l.spans.push(cap);
    }
    l
}

/// Controls wrapped whole; none disappear when the page is narrow
/// (keysControls).
pub fn controls(w: usize, pairs: &[(&str, &str)]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut cur = Line::new();
    for (k, what) in pairs {
        let item = line([spb(*k, TEXT), plain(" "), sp(*what, TEXT)]);
        if cur.width() > 0 && cur.width() + 3 + item.width() > w {
            out.push(std::mem::take(&mut cur));
        }
        if cur.width() > 0 {
            cur.spans.push(plain(" · "));
        }
        join(&mut cur, item);
    }
    if cur.width() > 0 {
        out.push(cur);
    }
    out
}

/// What's being practiced, when it's bound to something.
fn practicing(m: &Model, km: &Map) -> String {
    let p = &dialog(m).keys.practice;
    if practiced(km, p).is_empty() {
        String::new()
    } else {
        p.join(" ")
    }
}

/// The page, w wide (keysBody).
pub fn body(m: &Model, geo: &mut Geometry, w: usize, _h: usize) -> Vec<Line> {
    let km = key_map(m);
    let d = dialog(m);
    let kd = &d.keys;
    let ctxs = places(&km);
    let ctx = context(&km, kd);
    let rows = rows(&km, kd);
    let cursor = d.cursor.min(rows.len().saturating_sub(1));
    // Frame chrome plus the page's title.
    let budget = (m.h as isize - (m.top_h() as isize - 2) - 6).max(1) as usize;
    let title = ctx.map(Context::title).unwrap_or("");
    let mut out = vec![line([
        spb(title, TEXT),
        sp(format!(" · {}/{}", kd.ctx + 1, ctxs.len()), DIM),
        plain("  "),
        sp("← → change context", TEXT),
    ])];
    for l in wrap(ctx.map(context_where).unwrap_or(""), w) {
        out.push(line([sp(l, TEXT)]));
    }
    let mut footer = controls(
        w,
        &[
            ("enter", "Edit binding"),
            ("a", "Add alternative"),
            ("x", "Unbind"),
            ("r", "Reset default"),
        ],
    );
    footer.extend(controls(
        w,
        &[
            ("↑ ↓", "Select action"),
            ("[ ]", "Settings page"),
            ("esc", "Back"),
        ],
    ));
    if let Some(a) = rows.get(cursor) {
        let mut detail = line([sp(a.title.clone(), TEXT), plain(" · ")]);
        join(&mut detail, key_caps(km.keys(&a.id), ""));
        let mut f = wrap_line(&detail, w);
        f.extend(footer);
        footer = f;
    }
    if budget >= 20 {
        footer.insert(
            0,
            line([sp(
                "Try a shortcut to find its action. • marks a custom binding.",
                DIM,
            )]),
        );
    }
    let problems = km.problems();
    if let Some(p) = problems.first() {
        let s = format!("{} invalid bindings: {p}", problems.len());
        for l in wrap(&s, w) {
            out.push(line([sp(l, YELLOW)]));
        }
    }
    let practicing = practicing(m, &km);
    let taking = m
        .kept
        .peek::<State>()
        .map(|s| s.keys.taking.clone())
        .unwrap_or_default();
    let keys_of = |a: &Action| -> Line {
        if !taking.is_empty() && taking == a.id {
            return line([sp("press keys…", ORANGE)]);
        }
        key_caps(km.keys(&a.id), &practicing)
    };
    // Keys first, as wide as the widest, so the eye runs down them.
    let (mut key_w, mut title_w) = (8usize, 6usize);
    for a in &rows {
        key_w = key_w.max(keys_of(a).width());
        title_w = title_w.max(width(&a.title));
    }
    let key_w = key_w.min(12.max(30.min(w / 3)));
    let mut action_w = 8.max(w as isize - key_w as isize - 5) as usize;
    let table_w = w.min(2 + key_w + 3 + title_w);
    // The keyboard never takes an action row: beside the table where the
    // screen is wide, else under it when every row already shows.
    let kb = board(
        m,
        &km,
        &rows,
        cursor,
        &practicing,
        w as isize - table_w as isize - 4,
    );
    let side = w as isize - table_w as isize - 4 >= KB_WIDE as isize;
    let mut row_w = w;
    if side {
        action_w = title_w;
        row_w = table_w;
    }
    out.push(line([spb(
        format!("  {}   ACTION", fit_str("BINDING", key_w)),
        TEXT,
    )]));
    // On very short screens keep the action and editing controls first.
    if out.len() + footer.len() + 2 > budget && out.len() > 2 {
        let last = out.pop().expect("heading");
        out.truncate(1);
        out.push(last);
    }
    if out.len() + footer.len() + 2 > budget && footer.len() > 2 {
        let keep = 1.max(budget as isize - out.len() as isize - 2) as usize;
        let cut = footer.len().saturating_sub(keep);
        footer.drain(..cut);
    }
    // The footer's room first; the actions get the rest.
    let n = 1.max(budget as isize - out.len() as isize - footer.len() as isize - 1) as usize;
    let (from, to) = window(rows.len(), cursor, n);
    let key_top = out.len();
    geo.list_top = 2 + key_top;
    geo.list_from = from;
    geo.list_to = to;
    geo.width = w;
    for (i, a) in rows.iter().enumerate().take(to).skip(from) {
        let mut l = if km.changed(&a.id) {
            line([sp("• ", BLUE)])
        } else {
            line([plain("  ")])
        };
        join(&mut l, fit(keys_of(a), key_w));
        l.spans.push(plain("   "));
        join(&mut l, fit(line([sp(a.title.clone(), TEXT)]), action_w));
        if i == cursor {
            l = highlight(l, row_w);
        } else if i + 1 == kd.hover {
            l = hover_line(l, row_w);
        }
        out.push(l);
    }
    out.push(line([sp(
        format!("{}–{} of {} actions", from + 1, to, rows.len()),
        DIM,
    )]));
    if side && to - from + 2 >= kb.len() {
        for (j, l) in kb.into_iter().enumerate() {
            let y = key_top - 1 + j;
            let mut r = fit(std::mem::take(&mut out[y]), table_w);
            r.spans.push(plain("    "));
            join(&mut r, l);
            out[y] = r;
        }
    } else if from == 0
        && to == rows.len()
        && out.len() + kb.len() + 1 + footer.len() <= budget
        && w >= KB_WIDE + 2
    {
        out.push(Line::new());
        for l in kb {
            let mut r = line([plain("  ")]);
            join(&mut r, l);
            out.push(r);
        }
    }
    out.extend(footer);
    out.into_iter().map(|l| fit(l, w)).collect()
}

/// The keyboard for rows: the hovered or selected action's keys lit, the
/// key being practiced brightest, every key bound here plain (keysBoard).
fn board(
    m: &Model,
    km: &Map,
    rows: &[Action],
    cursor: usize,
    practicing: &str,
    w: isize,
) -> Vec<Line> {
    let kd = &dialog(m).keys;
    let lit: &[Seq] = if kd.hover > 0 && kd.hover <= rows.len() {
        km.keys(&rows[kd.hover - 1].id)
    } else if let Some(a) = rows.get(cursor) {
        km.keys(&a.id)
    } else {
        &[]
    };
    let hit = if practicing.is_empty() {
        HashSet::new()
    } else {
        key_parts(&kd.practice)
    };
    let mut on = HashSet::new();
    for s in lit {
        on.extend(key_parts(s.keys()));
    }
    let mut used = HashSet::new();
    for a in rows {
        for s in km.keys(&a.id) {
            used.extend(key_parts(s.keys()));
        }
    }
    let mut out = keyboard(&hit, &on, &used);
    out.push(Line::new());
    out.push(fit(legend(), KB_WIDE.max(w.max(0) as usize)));
    out
}

/// The keys being taken asked for in a box over the screen (keysModal).
pub fn modal(m: &mut Model, base: &mut Vec<Line>) {
    let Some(st) = m.kept.peek::<State>() else {
        return;
    };
    if st.keys.taking.is_empty() || st.dialog.is_none() {
        return;
    }
    let (taking, adding, pressed) = (
        st.keys.taking.clone(),
        st.keys.adding,
        st.keys.pressed.clone(),
    );
    let km = key_map(m).into_owned();
    let Some(a) = km.action(&taking).cloned() else {
        return;
    };
    let bw = m.w.saturating_sub(4).min(64);
    let verb = if adding {
        "Another key for "
    } else {
        "New keys for "
    };
    let pressed: Span = if pressed.is_empty() {
        sp("…", FAINT)
    } else {
        keycap(&pressed.join(" "), true)
    };
    let mut now = line([sp(fit_str("Now", 10), DIM)]);
    join(&mut now, key_caps(km.keys(&a.id), ""));
    let mut body = vec![
        line([spb(format!("{verb}{}", a.title), TEXT)]),
        line([sp(a.id.clone(), FAINT)]),
        Line::new(),
        now,
        line([sp(fit_str("Pressed", 10), DIM), pressed]),
        Line::new(),
    ];
    let inner = bw.saturating_sub(4);
    for l in wrap(
        "Press the key, or up to three for a chord (ctrl+x then p, say).",
        inner,
    ) {
        body.push(line([sp(l, DIM)]));
    }
    if a.context != Context::Global {
        for l in wrap(
            "A key that types a character, p say, can't be one on its own here: start a chord with it instead.",
            inner,
        ) {
            body.push(line([sp(l, DIM)]));
        }
    }
    body.push(Line::new());
    body.push(keys(&[("enter", "keep"), ("esc", "cancel")]));
    m.modal_over(base, &body, bw, ORANGE);
}

/// A key on Keys (keysKey).
pub fn key(m: &mut Model, s: &str) -> Out {
    let km = key_map(m).into_owned();
    let d = dialog(m);
    let rows = rows(&km, &d.keys);
    let Some(a) = rows
        .get(d.cursor.min(rows.len().saturating_sub(1)))
        .cloned()
    else {
        return Out::Idle;
    };
    match s {
        "left" | "right" | "h" | "l" => {
            let n = places(&km).len().max(1);
            let back = s == "left" || s == "h";
            let d = dialog_mut(m);
            let cur = d.keys.ctx.min(n - 1);
            d.keys.ctx = if back {
                (cur + n - 1) % n
            } else {
                (cur + 1) % n
            };
            d.cursor = 0;
            Out::Draw
        }
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
            let i = (s.as_bytes()[0] - b'1') as usize;
            if i < places(&km).len() {
                let d = dialog_mut(m);
                d.keys.ctx = i;
                d.cursor = 0;
            }
            Out::Draw
        }
        "enter" | "a" => {
            let k = &mut m.kept::<State>().keys;
            k.taking = a.id.clone();
            k.adding = s == "a";
            k.pressed.clear();
            Out::Draw
        }
        "x" | "backspace" | "delete" => save_binding(m, &a.id, Some(Vec::new())),
        "r" => save_binding(m, &a.id, None),
        _ => practice_key(m, s),
    }
}

/// Each key pressed while keys are being taken: every key comes here
/// first, unmapped (takeKey, through the frame's keys hook).
pub fn capture(m: &mut Model, k: &Press) -> Out {
    let taking = m
        .kept
        .peek::<State>()
        .is_some_and(|s| !s.keys.taking.is_empty() && s.dialog.is_some());
    if !taking {
        return Out::Pass;
    }
    take_key(m, &k.s)
}

fn take_key(m: &mut Model, s: &str) -> Out {
    let n = m.kept::<State>().keys.pressed.len();
    if s == "esc" {
        clear_taking(m);
        return Out::Draw;
    }
    if s == "enter" && n > 0 {
        return taken(m);
    }
    m.kept::<State>().keys.pressed.push(s.to_string());
    if n + 1 == 3 {
        return taken(m);
    }
    Out::Draw
}

fn clear_taking(m: &mut Model) {
    let k = &mut m.kept::<State>().keys;
    k.taking.clear();
    k.adding = false;
    k.pressed.clear();
}

/// The keys taken: bound, or asked about first when they're another's.
fn taken(m: &mut Model) -> Out {
    let k = &m.kept::<State>().keys;
    let (id, seq, adding) = (k.taking.clone(), k.pressed.clone(), k.adding);
    clear_taking(m);
    if seq.is_empty() {
        return Out::Draw;
    }
    let km = key_map(m).into_owned();
    let mut keys: Vec<String> = Vec::new();
    if adding {
        keys.extend(km.keys(&id).iter().map(Seq::to_string));
    }
    let seq = Seq(seq);
    keys.push(seq.to_string());
    let first = &seq.0[0];
    if let Some(a) = km.action(&id)
        && rush_keymap::types(first)
        && a.context != Context::Global
        && !a.keys.contains(first)
    {
        m.flash_err(&format!(
            "{first} types a character, so it can't start a key: make it a chord, ctrl+x then {first} say"
        ));
        return Out::Draw;
    }
    let c = km.conflicts(&id, &seq);
    if !c.is_empty() {
        let q = format!("{seq} is {}'s now: take it?", c.join(", "));
        m.ask(Confirm::new(q, move |m| save_binding(m, &id, Some(keys))));
        return Out::Draw;
    }
    save_binding(m, &id, Some(keys))
}

/// Sets id's keys (None for rush's) and writes keybindings.json, off the
/// UI (saveBinding).
pub fn save_binding(m: &mut Model, id: &str, keys: Option<Vec<String>>) -> Out {
    let f = m.kept::<State>().keys.file.with(id, keys.clone());
    set_keys(m, f.clone());
    match &keys {
        None => m.flash(&format!("{id}: rush's keys again")),
        Some(k) if k.is_empty() => m.flash(&format!("{id}: no key")),
        Some(_) => {
            let t = key_map(m).key_text(id);
            m.flash(&format!("{id}: {t}"));
        }
    }
    let path = m
        .kept::<State>()
        .keys
        .path
        .clone()
        .unwrap_or_else(rush_keymap::path);
    m.later(
        move || rush_keymap::save_to(&path, &f).map_err(|e| e.to_string()),
        |m, r| {
            if let Err(e) = r {
                m.flash_err(&format!("keybindings.json: {e}"));
                return Out::Draw;
            }
            Out::Idle
        },
    );
    Out::Draw
}

/// A key pressed on Keys that isn't the page's own, tried against every
/// action's keys, so a key can be pressed to see what it's bound to: a
/// match jumps the page to it and lights its chip, a chord's first key
/// waits for the rest, a miss clears (practiceKey).
fn practice_key(m: &mut Model, s: &str) -> Out {
    let km = key_map(m).into_owned();
    let now = Instant::now();
    let d = dialog_mut(m);
    if d.keys.practice_at.is_none_or(|t| now - t > CHORD_WAIT) {
        d.keys.practice.clear();
    }
    d.keys.practice_at = Some(now);
    // The key pressed shows over the row the pointer rests on, till it
    // moves.
    d.keys.hover = 0;
    let try_seq = |m: &mut Model, seq: Vec<String>| -> bool {
        let hits = practiced(&km, &seq);
        if let Some(a) = hits.first() {
            let id = a.id.clone();
            dialog_mut(m).keys.practice = seq;
            show_key(m, &id);
            return true;
        }
        if seq.len() < 3 && practiced_prefix(&km, &seq) {
            dialog_mut(m).keys.practice = seq;
            return true;
        }
        false
    };
    let mut seq = dialog(m).keys.practice.clone();
    seq.push(s.to_string());
    if !try_seq(m, seq) && !try_seq(m, vec![s.to_string()]) {
        dialog_mut(m).keys.practice.clear();
    }
    Out::Draw
}

/// The actions whose keys are seq, exactly (practiced).
pub fn practiced<'a>(km: &'a Map, seq: &[String]) -> Vec<&'a Action> {
    if seq.is_empty() {
        return Vec::new();
    }
    let s = seq.join(" ");
    km.actions()
        .into_iter()
        .filter(|a| km.keys(&a.id).iter().any(|k| k.to_string() == s))
        .collect()
}

/// Whether seq begins a longer chord bound to something
/// (practicedPrefix).
pub fn practiced_prefix(km: &Map, seq: &[String]) -> bool {
    let s = format!("{} ", seq.join(" "));
    km.actions()
        .into_iter()
        .any(|a| km.keys(&a.id).iter().any(|k| k.to_string().starts_with(&s)))
}

/// The row of Keys under the pointer at y in the body's cells; whether
/// that changed (keysHover).
pub fn hover(m: &mut Model, g: &Geometry, y: usize) -> bool {
    let i = y as isize - g.list_top as isize + g.list_from as isize;
    let h = if i >= g.list_from as isize && i < g.list_to as isize {
        i as usize + 1
    } else {
        0
    };
    let d = dialog_mut(m);
    let changed = h != d.keys.hover;
    d.keys.hover = h;
    changed
}
