//! Agents and the Session side by side: the list keeps its share on the
//! left and a Session shows beside it, the opened one with the keys or
//! the picked agent's without; shift+← →, ctrl+6 and #view move between
//! the split, the list alone and the Session alone. After Go's
//! internal/ui/view.go (widths :881, sideWidth :917, maxPane :933,
//! stepSplit :951, chatOpen :981, chatAlone :985, listOnly :991,
//! splitAgain :1000, canSplit :1017, splitHint :1021, sessionOnly :1042,
//! minPane :1078, listView :1098's rows side by side, fadeRow :1359,
//! divider :1469), model.go (wide :1920, autoSplit :1924), keys.go:86 (a
//! Session filling a narrow screen has the keys), rushmode.go:5207
//! (leavePane, leaveChat), panegeometry.go:13 (paneFrameRow), keys.go:1068
//! (#view) and hosted.go:133 (toggleList: ctrl+6 outside the hosted view).
//! The list's edge dragged with the mouse: view.go:1055 (dragSplit),
//! model.go:1528-1560 (following it, divHover) and :1658 (grabbing it).

use rush_core::text::Line;

use crate::frame::{Out, Press};
use crate::model::Model;
use crate::session;
use crate::style::*;

/// The narrowest pane worth splitting the screen for (minPane).
pub const MIN_PANE: usize = 84;
/// The widest a Session gets: its rows cap at 124 columns, plus the
/// pane's margins (maxPane).
pub const MAX_PANE: usize = 128;

/// The screen's width: the model's, but while the opened Session beside
/// the list is drawn or takes a key, the model is narrowed to the pane
/// and the screen is kept aside (Model::screen_w).
pub fn sw(m: &Model) -> usize {
    if m.in_pane { m.screen_w } else { m.w }
}

/// Runs f with the model narrowed to the pane's width, pw, the screen's
/// kept aside: so the opened Session beside the list lays itself out,
/// takes keys and the mouse as it was drawn.
pub fn in_pane<T>(m: &mut Model, pw: usize, f: impl FnOnce(&mut Model) -> T) -> T {
    if m.in_pane {
        return f(m);
    }
    let full = m.w;
    m.screen_w = full;
    m.w = pw;
    m.in_pane = true;
    let out = f(m);
    m.in_pane = false;
    m.w = full;
    out
}

/// A screen wide enough to split without being asked (wide).
pub fn wide(w: usize) -> bool {
    w >= 170
}

/// Whether the Session shows beside the list without being asked: a wide
/// screen you haven't pushed to the list alone (autoSplit).
pub fn auto_split(m: &Model) -> bool {
    wide(sw(m)) && !m.cfg.list_only
}

/// Whether the screen is wide enough for Agents and the Session side by
/// side (canSplit).
pub fn can_split(w: usize) -> bool {
    w.saturating_sub(floor(w) + 1) >= MIN_PANE
}

/// The list's least share: a quarter of the screen, 30 columns at least.
fn floor(w: usize) -> usize {
    w.div_ceil(4).max(30)
}

/// The list's width in a split: your own share when you've set one,
/// else rush's; never under a quarter of the screen or 30 columns
/// (sideWidth).
pub fn side_width(m: &Model) -> usize {
    let w = sw(m);
    let floor = floor(w);
    let side = floor.max((w * 28 / 100).min(64));
    let f = m.cfg.side_width;
    if f > 0.0 {
        // A width you dragged to wins.
        let want = (w as f64 * f + 0.5) as usize;
        return floor.max(want.min(w * 3 / 4));
    }
    // The Session never takes more than its content can use; the spare
    // room goes to Agents.
    side.max(w.saturating_sub(1 + MAX_PANE))
}

/// Go's preview: a Session on screen, the opened one or the one ctrl+6
/// put beside the list for now.
pub fn preview(m: &Model) -> bool {
    m.session.is_some() || previewing(m)
}

/// Whether a Session is on screen, beside the list or alone (chatOpen).
pub fn chat_open(m: &Model) -> bool {
    m.full || preview(m) || auto_split(m)
}

/// Whether the Session has the whole screen: pushed there, or opened
/// with the Session alone as your layout (chatAlone).
pub fn chat_alone(m: &Model) -> bool {
    m.full || preview(m) && m.cfg.list_only && m.cfg.chat_full
}

/// How Go splits the screen (widths): (list, pane), the list 0 when the
/// Session has the screen, the pane 0 with the list alone. It may give
/// the Session the screen with none opened yet: `settle` opens it.
fn go_widths(m: &Model) -> (usize, usize) {
    let w = sw(m);
    if crate::hosted::alone(m) {
        return (0, w); // the one session, full width
    }
    if crate::hosted::id(m).is_some() {
        // Hosted's list, beside the session when there's room for both.
        let side = side_width(m);
        if w.saturating_sub(side + 1) >= MIN_PANE {
            return (side, w - side - 1);
        }
        return (w, 0);
    }
    if m.zen().is_some() {
        return (0, w); // zen is the one agent, full width
    }
    if !chat_open(m) {
        return (w, 0);
    }
    // The list keeps at least a quarter of the screen and 30 columns; if
    // that leaves the pane too narrow to read, there's no split.
    let side = side_width(m);
    if chat_alone(m) {
        (0, w)
    } else if w.saturating_sub(side + 1) >= MIN_PANE {
        (side, w - side - 1)
    } else if preview(m) {
        (0, w)
    } else {
        (w, 0)
    }
}

/// How the screen splits between the list and the Session beside it:
/// (list, pane). The list 0 is the opened Session alone; the pane 0 the
/// list alone (widths). The Session has the screen only once one is
/// opened: till then, the list (`settle` opens it).
pub fn widths(m: &Model) -> (usize, usize) {
    match go_widths(m) {
        (0, _) if m.session.is_none() => (sw(m), 0),
        lp => lp,
    }
}

/// Opens the Session that is to fill the screen: one beside the list on
/// a screen too narrow for both, or with the Session alone as your
/// layout. "A Session filling a narrow screen has the only box there
/// is, so the keys are its" (keys.go:86).
pub fn settle(m: &mut Model) {
    if m.session.is_some() || !previewing(m) || go_widths(m).0 > 0 {
        return;
    }
    let key = m.sel.clone();
    if crate::screens::social::rooms::is_room_key(&key) {
        crate::screens::social::rooms::open_room(m, &key);
    } else if let Some(i) = m.focused() {
        m.open(i);
    }
    if m.session.is_none() {
        end_preview(m);
    }
}

/// Whether the list and the Session are side by side now.
pub fn side_by_side(m: &Model) -> bool {
    let (l, p) = widths(m);
    l > 0 && p > 0
}

/// The opened Session gives the list the keys back (leavePane): beside
/// the list it stays there, where it was; with no list beside it, the
/// list comes back in its place (leaveChat).
pub fn leave_pane(m: &mut Model) {
    if side_by_side(m) {
        if let Some(v) = m.session.take() {
            m.pane = Some(v);
            m.kept::<Preview>().0 = true;
        }
        return;
    }
    leave_chat(m);
}

/// The Session closes for Agents (leaveChat): from the Session alone,
/// kept that way, it's a peek; your layout comes back, not what ctrl+6
/// showed.
pub fn leave_chat(m: &mut Model) {
    let alone = m.session.is_some() && chat_alone(m);
    super::peek::leave_chat(m, alone);
    m.session = None; // the tail's worker stops with its handle
    m.full = false;
    end_preview(m);
}

/// The key back to the split, for the row of keys, while one side has
/// the screen and there's room for both (splitHint).
pub fn hint(m: &Model) -> Option<(String, String)> {
    if m.zen().is_some() || crate::hosted::id(m).is_some() || !can_split(sw(m)) {
        return None;
    }
    let (l, p) = widths(m);
    if l > 0 && p > 0 {
        return None;
    }
    let back = if l == 0 { "shift+→" } else { "shift+←" };
    Some((
        format!("{back} · #view split"),
        "back to side by side".into(),
    ))
}

/// Keeps the layout: in the list's config at once, on disk in order with
/// every other change, or with the width when a drag did it
/// (Config.SetView, SaveConfig). Agents alone keeps how a Session
/// opens from it (listOnly sets View and ListOnly only).
fn keep_view(m: &mut Model, v: &'static str) {
    let set = move |view: &mut String, list_only: &mut bool, chat_full: &mut bool| {
        *view = v.into();
        *list_only = v != "split";
        if v != "list" {
            *chat_full = v == "agent";
        }
    };
    set(&mut m.cfg.view, &mut m.cfg.list_only, &mut m.cfg.chat_full);
    if m.kept::<Edge>().dragging {
        return; // kept with the width once the edge is let go
    }
    crate::config::save(m, "couldn't keep the layout", move |c| {
        set(&mut c.view, &mut c.list_only, &mut c.chat_full);
    });
}

/// The Session beside the list for now, though your layout is the list
/// alone: ctrl+6 brought it, or the opened one gave the list the keys
/// back (Go's preview, with no Session opened).
#[derive(Default)]
struct Preview(bool);

/// Shows the Session beside the list, where it isn't already (v on an
/// agent's screen, Go's m.preview = true).
pub fn show(m: &mut Model) {
    if widths(m).1 == 0 {
        m.kept::<Preview>().0 = true;
    }
}

fn previewing(m: &Model) -> bool {
    m.kept.peek::<Preview>().is_some_and(|p| p.0)
}

/// Lets go of the Session ctrl+6 put beside the list (leaveChat's
/// preview).
pub fn end_preview(m: &mut Model) {
    if previewing(m) {
        m.kept::<Preview>().0 = false;
    }
}

/// Opens the picked agent, or room, with the keys (focusPane).
fn open_picked(m: &mut Model) {
    let key = m.sel.clone();
    if crate::screens::social::rooms::is_room_key(&key) {
        crate::screens::social::rooms::open_room(m, &key);
    } else if let Some(i) = m.focused() {
        m.open(i);
    }
}

/// ctrl+6 outside the hosted view (toggleList): from the split the
/// Session takes the screen, with the keys; from the Session alone, or
/// the list alone, the two come side by side when there's room, the list
/// with the keys, else the list has the screen. The layout #view keeps
/// isn't changed.
pub fn toggle(m: &mut Model) -> Out {
    let room = crate::screens::social::rooms::is_room_key(&m.sel);
    if !room && m.focused().is_none() {
        return Out::Idle;
    }
    if widths(m).0 > 0 && chat_open(m) {
        m.full = true;
        if m.session.is_none() {
            open_picked(m);
        }
        return Out::Draw;
    }
    m.full = false;
    super::peek::clear(m);
    if !chat_open(m) {
        m.kept::<Preview>().0 = true;
    }
    if go_widths(m).0 == 0 {
        // No room for both, or the Session alone is your layout: the list
        // takes the screen.
        leave_chat(m);
        return Out::Draw;
    }
    // The keys to the list; the Session stays beside it as it is,
    // scrolled where it was.
    if let Some(v) = m.session.take() {
        m.pane = Some(v);
        m.kept::<Preview>().0 = true;
    }
    Out::Draw
}

/// Hides the Session and gives the screen to Agents, and keeps it that
/// way, next time too: a Session opens from there as you last had one
/// (listOnly).
pub fn list_only(m: &mut Model) {
    keep_view(m, "list");
    m.session = None;
    m.full = false;
    end_preview(m);
    m.pane = None;
    m.flash("just Agents, next time too · shift+← or #view split puts the Session beside them");
}

/// Agents and the Session side by side, when there's room, next time too
/// (splitAgain). An opened Session stays open, beside the list.
pub fn split_again(m: &mut Model) {
    if !can_split(sw(m)) {
        m.flash_err("too narrow for Agents and the Session side by side");
        return;
    }
    if !chat_open(m) {
        m.kept::<Preview>().0 = !wide(sw(m));
    }
    keep_view(m, "split");
    m.full = false;
    m.flash("Agents and the Session side by side, next time too");
}

/// The screen to the picked agent's Session, next time too: Sessions
/// open alone from then on (sessionOnly).
pub fn session_only(m: &mut Model) {
    let Some(i) = m.selected() else { return };
    keep_view(m, "agent");
    m.full = true;
    if m.session.as_ref().is_none_or(|v| v.key != m.agents[i].key) {
        m.open(i);
    }
    m.flash("just the Session, next time too · esc for Agents · shift+→ or #view split for both side by side");
}

/// The list's share, kept between a quarter and three quarters, never
/// so wide the split goes (setSideWidth).
fn set_side_width(m: &mut Model, cols: usize) {
    if sw(m) == 0 {
        return;
    }
    let cols = cols.min(sw(m).saturating_sub(1 + MIN_PANE));
    let f = (cols as f64 / sw(m) as f64).clamp(0.25, 0.75);
    m.cfg.side_width = f;
    m.flash(&format!("list width {:.0}% · release to keep", f * 100.0));
}

/// The list's edge and the mouse: being dragged, or under the pointer
/// (Go's dragging, divHover).
#[derive(Default)]
struct Edge {
    dragging: bool,
    hover: bool,
}

/// Whether the edge shows a shade brighter: dragged, or the mouse on it
/// (divider).
pub fn divider_lit(m: &Model) -> bool {
    m.kept.peek::<Edge>().is_some_and(|e| e.dragging || e.hover)
}

/// Whether column x is on the edge between the list and the Session
/// beside it (model.go:1658: the divider, or the column after it).
fn on_edge(m: &Model, x: usize) -> bool {
    let (lw, pw) = widths(m);
    lw > 0 && pw > 0 && (x == lw || x == lw + 1)
}

/// The mouse on the list's edge, before anything else has it: pressed
/// there, the edge follows the mouse until it's let go, and is kept then;
/// moving over it lights it (model.go:1528-1560, :1658). Some when the
/// edge took the mouse; else whether the edge's look changed, so the
/// frame is drawn again whatever else the mouse does.
pub fn edge_mouse(m: &mut Model, e: &crate::frame::Mouse) -> Result<Out, bool> {
    use rush_draw::{Button, MouseKind};
    let x = e.x as usize;
    if m.kept::<Edge>().dragging {
        if let MouseKind::Drag(Button::Left) = e.kind {
            drag_split(m, x);
            return Ok(Out::Draw);
        }
        m.kept::<Edge>().dragging = false;
        keep_width(m);
        if matches!(e.kind, MouseKind::Up(_)) {
            return Ok(Out::Draw);
        }
        return Err(true);
    }
    // A box over the screen (a sheet, a question, ctrl+k, the keys' help,
    // a menu) has the mouse, as it has the keys (overlayClick).
    let free = !m.on_place()
        && m.confirm.is_none()
        && m.sheet.is_none()
        && !m.over.on
        && !crate::screens::cmdbar::keyhelp::is_open(m)
        && !crate::screens::cmdbar::bar::is_open(m);
    let on = free && e.y as usize >= m.top_h() && on_edge(m, x);
    match e.kind {
        MouseKind::Down(Button::Left) if on => {
            // The quick ask and the btw boxes float over the edge: a
            // click on one is theirs (clickQuick, clickBox come first).
            use crate::screens::session_cards::{btw, quickask};
            for f in [quickask::quick_mouse, btw::btw_mouse] {
                let o = f(m, e);
                if o.used() {
                    return Ok(o);
                }
            }
            m.kept::<Edge>().dragging = true;
            Ok(Out::Draw)
        }
        MouseKind::Moved => {
            let edge = m.kept::<Edge>();
            let changed = edge.hover != on;
            edge.hover = on;
            Err(changed)
        }
        _ => Err(false),
    }
}

/// Follows the list's edge as it's dragged to column x; dropped against
/// either side of the screen, that side's pane goes (dragSplit).
fn drag_split(m: &mut Model, x: usize) {
    let w = sw(m);
    if x + 2 >= w {
        if chat_open(m) {
            list_only(m);
        }
    } else if x <= 1 {
        if !chat_alone(m) {
            session_only(m);
        }
    } else {
        if !chat_open(m) {
            m.kept::<Preview>().0 = !wide(w);
        }
        m.full = false;
        (m.cfg.view, m.cfg.list_only, m.cfg.chat_full) = ("split".into(), false, false);
        set_side_width(m, x);
    }
}

/// Keeps the layout and the list's width, once the edge is let go: one
/// save for the whole drag, whatever it did on the way (SaveConfig).
fn keep_width(m: &mut Model) {
    let (view, list_only, chat_full, side) = (
        m.cfg.view.clone(),
        m.cfg.list_only,
        m.cfg.chat_full,
        m.cfg.side_width,
    );
    crate::config::save(m, "couldn't keep the layout", move |c| {
        (c.view, c.list_only, c.chat_full, c.side_width) =
            (view.clone(), list_only, chat_full, side);
    });
}

/// #view split|agent|list (keys.go:1068).
pub fn command(m: &mut Model, arg: &str) -> Out {
    match arg.trim() {
        "split" => split_again(m),
        "agent" | "chat" | "session" => session_only(m),
        "list" | "agents" | "orchestrator" => list_only(m),
        _ => m.flash_err("view one of: split, agent, list"),
    }
    Out::Draw
}

/// shift+← and shift+→ in the list (alt too) move the list's edge two
/// columns; pushed past either end, the screen goes to the Session alone
/// or the list alone; from one side alone, the other comes back beside
/// it (stepSplit).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let grow = match k.s.as_str() {
        "shift+right" | "alt+right" => true,
        "shift+left" | "alt+left" => false,
        _ => return Out::Pass,
    };
    let w = sw(m);
    let floor = floor(w);
    if w.saturating_sub(floor + 1) < MIN_PANE || m.zen().is_some() || crate::hosted::id(m).is_some()
    {
        return Out::Pass; // too narrow for a split to step through
    }
    let ceil = (w * 3 / 4).min(w.saturating_sub(1 + MIN_PANE));
    let side = side_width(m);
    if widths(m).0 == 0 {
        if grow {
            split_again(m);
        }
    } else if !chat_open(m) {
        if !grow {
            split_again(m);
        }
    } else if grow && side + 2 > ceil {
        list_only(m);
    } else if !grow && side < floor + 2 {
        session_only(m);
    } else if grow {
        set_side_width(m, side + 2);
    } else {
        set_side_width(m, side - 2);
    }
    Out::Draw
}

/// The agent last found with no conversation to read, so it isn't tried
/// again every frame.
#[derive(Default)]
struct Unreadable(String);

/// Keeps the Session beside the list on the agent picked: read again
/// only when the pick moves to another (loadPreview).
pub fn follow(m: &mut Model) {
    // A room's row: its log drawn as its agents' conversation (syncHost's
    // syncRoom).
    if crate::screens::social::rooms::is_room_key(&m.sel) {
        let key = m.sel.clone();
        if m.pane.as_ref().is_none_or(|v| v.key != key) {
            m.pane = crate::screens::social::rooms::room_view(m, &key);
        }
        return;
    }
    let Some(i) = m.focused() else {
        m.pane = None;
        return;
    };
    let key = m.agents[i].key.clone();
    if m.pane.as_ref().is_some_and(|v| v.key == key) {
        return;
    }
    if m.pane.is_none() && m.kept::<Unreadable>().0 == key {
        return;
    }
    m.pane = m.read_view(i, true);
    m.kept::<Unreadable>().0 = if m.pane.is_none() { key } else { String::new() };
    // A live session rush hosts is its host's: dialled now (syncHost).
    crate::screens::session_core::live::sync(m);
}

/// n cells of the header's chrome.
fn chrome(n: usize) -> rush_core::text::Span {
    let st = rush_core::text::Style {
        bg: CHROME_BG,
        ..Default::default()
    };
    rush_core::text::Span::new(blanks(n), st)
}

/// The pane of a session rush hosts while its host isn't reached yet:
/// its frame's chrome round a line saying so (rushPane's placeholder in
/// paneFrameRow's gutters); None once it's connected or read.
fn connecting(m: &mut Model, v: &session::SessionView, w: usize, rows: usize) -> Option<Vec<Line>> {
    if !matches!(v.source, session::Source::Hosted) || !v.sess.turns.is_empty() {
        return None;
    }
    let a = m.agents.iter().find(|a| a.key == v.key)?;
    let name = one_line(&a.display_name);
    let say = if a.pid == 0 {
        format!("  {name} isn't running, and has no conversation to show yet")
    } else {
        format!("  connecting to {name}…")
    };
    let c = crate::screens::session_core::conn(m, &v.key);
    if c.link.is_some() {
        return None;
    }
    let inner = w.saturating_sub(3);
    let body = [Line::new(), line([sp(say, DIM)])];
    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let mid = fit(body.get(i).cloned().unwrap_or_default(), inner);
        let mut r = match i {
            0..4 => line([chrome(2)]),
            4 => line([sp("──", FAINT)]),
            _ => line([plain("  ")]),
        };
        join(&mut r, mid);
        r.spans.push(match i {
            0..4 => chrome(1),
            4 => sp("─", FAINT),
            _ => plain(" "),
        });
        out.push(r);
    }
    Some(out)
}

/// The picked agent's own screen, mirrored, w wide and rows tall, for
/// an agent outside rush mode while it shows (view.go:1225-1235:
/// rushPane is nil, so liveLines); None for its conversation instead.
/// Asks for the mirror, sized to the pane, every frame it could show.
pub fn mirror(m: &mut Model, w: usize, rows: usize) -> Option<Vec<Line>> {
    let i = m.focused()?;
    let a = m.agents[i].clone();
    super::live::want(m, &a.key, w, rows);
    super::live::pane(m, &a, w, rows)
}

/// What the pane says while there's no Session to draw in it, w wide
/// and rows tall: none read for the agent picked, or one rush hosts not
/// reached yet (rushPane's placeholder); None once there's one to draw.
pub fn placeholder(m: &mut Model, w: usize, rows: usize) -> Option<Vec<Line>> {
    let Some(v) = m.pane.take() else {
        let say = match m.focused() {
            Some(i) => format!(
                "  {} has no conversation to show yet",
                one_line(&m.agents[i].display_name)
            ),
            None => "  select an agent to see what it is doing".into(),
        };
        return Some(vec![Line::new(), line([sp(say, DIM)])]);
    };
    let out = connecting(m, &v, w, rows);
    m.pane = Some(v);
    out
}

/// The kind of the agent whose Session is beside the list, opened or
/// not: its harness tints the divider (sessionAgent).
pub fn pane_kind(m: &Model) -> Option<String> {
    let v = m.session.as_ref().or(m.pane.as_ref())?;
    let a = m.agents.iter().find(|a| a.key == v.key)?;
    Some(a.kind.as_str().to_string())
}

/// The edge between Agents and the Session: a quiet line, in the
/// harness's grey when a Session is there (divider).
pub fn divider(kind: Option<&str>, reg: &rush_core::adapter::Registry) -> Line {
    match kind {
        Some(k) => line([sp(
            "│",
            crate::screens::accounts::look::harness_border(reg, k),
        )]),
        None => line([sp("│", FAINT)]),
    }
}

/// How far the side without the keys fades back toward the ground
/// (view.go:1322 fadeBy).
const FADE_BY: f64 = 0.3;

/// A row on the side without the keys: every text colour faded toward
/// the ground, the terminal's own text too, and faint dropped, the fade
/// already being there; grounds stay (fadeRow, fadeCode).
pub fn fade_row(l: Line) -> Line {
    use crate::frame::ground::{Rgb, mix, painted};
    let g = painted();
    let ink = mix(g.fg, g.bg, FADE_BY).color();
    let spans = l
        .spans
        .into_iter()
        .map(|mut s| {
            s.style.fg = match Rgb::of(s.style.fg) {
                _ if s.style.fg == rush_core::text::Color::Default => ink,
                Some(c) => mix(c, g.bg, FADE_BY).color(),
                None => s.style.fg,
            };
            s.style.mods = rush_core::text::Mods(s.style.mods.0 & !rush_core::text::Mods::DIM.0);
            s
        })
        .collect();
    Line::from_spans(spans)
}

/// The list's rows and the pane's side by side, each fit to its width,
/// the list's faded when the Session has the keys, else the pane's
/// (listView's split2: only once a Session is there to take them).
#[allow(clippy::too_many_arguments)]
pub fn join_rows(
    left: &[Line],
    lw: usize,
    pane: &[Line],
    pw: usize,
    n: usize,
    fade_list: bool,
    kind: Option<&str>,
    lit: bool,
    reg: &rush_core::adapter::Registry,
) -> Vec<Line> {
    // Dragged, or the mouse on it, the edge is a shade brighter.
    let div = if lit {
        line([sp("│", SUB)])
    } else {
        divider(kind, reg)
    };
    let two_sided = kind.is_some();
    (0..n)
        .map(|i| {
            let mut l = fit(left.get(i).cloned().unwrap_or_default(), lw);
            // A row's own ground stops at the divider.
            if l.bg != rush_core::text::Color::Default {
                let bg = l.bg;
                l = on_ground(l, lw, bg);
                l.bg = rush_core::text::Color::Default;
            }
            if two_sided && fade_list {
                l = fade_row(l);
            }
            join(&mut l, div.clone());
            let mut p = fit(pane.get(i).cloned().unwrap_or_default(), pw);
            if two_sided && !fade_list {
                // The pane's gutters keep their colours; its rows fade
                // (paneFrameRow round paneRow).
                use crate::screens::session_core::paint::cut;
                let bg = p.bg;
                let mut f = cut(&p, 0, 2);
                join(&mut f, fade_row(cut(&p, 2, pw.saturating_sub(1))));
                join(&mut f, cut(&p, pw.saturating_sub(1), pw));
                f.bg = bg;
                p = f;
            }
            l.bg = p.bg;
            join(&mut l, p);
            l
        })
        .collect()
}
