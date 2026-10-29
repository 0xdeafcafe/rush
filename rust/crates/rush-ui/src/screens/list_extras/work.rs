//! What's happening over time, read from the open sessions' transcripts:
//! each session's line on the Projects place (its state, how far through
//! its tasks, how full its context) and the timelines behind it. After
//! Go's internal/ui/workstreams.go (workSince :22, workLoad :77,
//! onWorkTimelines :123, workTick :130, workAgents :139, workSession
//! :176, workSaid :232, pickRow :248; lanesLine :277 is wall.rs's). The
//! Projects place itself is the projects group's: it calls these. Not
//! here yet: put-away (Done) sessions, which rush's own state keeps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rush_core::adapter::{EventKind, Happening, Timeline, fill_of};
use rush_core::agent::Agent;
use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_fleet::Predicates;

use super::wall::{lanes_line, spin};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// How far back these pages look (workSince).
pub const WORK_SINCE: Ms = 24 * 3_600_000;
/// How often the timelines are read again while they're shown (workTick).
pub const WORK_EVERY: Duration = Duration::from_secs(4);

/// The timelines as last read (workState's tls, views, loading, loaded).
#[derive(Default)]
pub struct WorkState {
    /// By transcript path; the loader's alone (None while it has them).
    tls: Option<HashMap<String, Box<dyn Timeline>>>,
    /// What each session did, by agent key, oldest first.
    pub views: HashMap<String, Vec<Happening>>,
    pub loading: bool,
    pub loaded: Option<Instant>,
}

/// The sessions worth reading: open, busy or at work within the window
/// (workAgents).
pub fn work_agents(m: &Model) -> Vec<usize> {
    let now = m.now;
    (0..m.agents.len())
        .filter(|&i| {
            let a = &m.agents[i];
            !a.done
                && !a.past
                && (a.job.live() || a.busy() || a.pid != 0 || now - a.job.updated_at < WORK_SINCE)
        })
        .collect()
}

/// Reads what the open sessions' transcripts have gained, off the UI
/// thread (workLoad).
pub fn load(m: &mut Model) {
    let st = m.kept::<WorkState>();
    if st.loading {
        return;
    }
    st.loading = true;
    let mut tls = st.tls.take().unwrap_or_default();
    let mut paths: HashMap<String, (String, String)> = HashMap::new();
    for i in work_agents(m) {
        let a = &m.agents[i];
        if !a.job.transcript_path.is_empty() {
            paths.insert(
                a.key.clone(),
                (a.job.transcript_path.clone(), a.kind.as_str().to_string()),
            );
        }
    }
    let reg = m.reg.clone();
    let since = m.now - WORK_SINCE;
    m.later(
        move || {
            tls.retain(|p, _| paths.values().any(|(q, _)| q == p));
            let mut views = HashMap::new();
            for (key, (p, kind)) in &paths {
                if !tls.contains_key(p) {
                    let Some(t) = reg
                        .get(kind)
                        .and_then(|a| a.timeliner().map(|t| t.new_timeline()))
                    else {
                        continue;
                    };
                    tls.insert(p.clone(), t);
                }
                let tl = tls.get_mut(p).expect("just put in");
                tl.update(p, since);
                views.insert(key.clone(), tl.events(since));
            }
            (tls, views)
        },
        |m, (tls, views)| {
            let st = m.kept::<WorkState>();
            st.loading = false;
            st.loaded = Some(Instant::now());
            st.tls = Some(tls);
            st.views = views;
            Out::Draw
        },
    );
}

/// Reads again when the last reading is WORK_EVERY old; a shown page
/// calls it as it draws (workTick).
pub fn tick(m: &mut Model) {
    let due = m
        .kept::<WorkState>()
        .loaded
        .is_none_or(|t| t.elapsed() >= WORK_EVERY);
    if due {
        load(m);
    }
}

/// What a session last said: its summary, or its last turn's words when
/// that summary is only a synthetic reply (workSaid).
pub fn work_said(m: &Model, a: &Agent) -> String {
    let d = one_line(&a.job.detail);
    if !d.is_empty() && d != "No response requested." {
        return d;
    }
    m.kept
        .peek::<WorkState>()
        .and_then(|st| st.views.get(&a.key))
        .and_then(|ev| {
            ev.iter()
                .rev()
                .find(|e| e.kind == EventKind::Turn && e.run.is_empty())
        })
        .map(|e| one_line(&e.text))
        .unwrap_or_default()
}

/// A session in Projects: its state, then how far through its tasks it is
/// and how full its context, w wide (workSession).
pub fn work_session(m: &Model, a: &Agent, w: usize) -> Line {
    let now = m.now;
    let (marker, state) = if a.halted() {
        (
            sp("✗", RED),
            line([sp(
                format!("stopped · {}", one_line(&a.halt_reason(now))),
                RED,
            )]),
        )
    } else if a.needs_you() {
        let mut q = one_line(&a.job.needs);
        if q.is_empty() {
            q = one_line(&a.job.detail);
        }
        (sp("?", YELLOW), line([sp("asks: ", YELLOW), sp(q, TEXT)]))
    } else if a.your_turn(now) {
        (
            sp("◆", GREEN),
            line([sp("your turn · ", GREEN), sp(work_said(m, a), SUB)]),
        )
    } else if a.job.live() {
        let mut s = work_said(m, a);
        if let Some(p) = m.previews.get(&a.key)
            && !p.doing.is_empty()
        {
            s = one_line(&p.doing);
        }
        if s.is_empty() {
            s = "working".into();
        }
        (
            sp(spin(a.kind.as_str(), m.tick + a.job.id.len()), ORANGE),
            line([sp(s, TEXT)]),
        )
    } else if a.busy() {
        (sp("◎", BLUE), line([sp(lanes_line(a), BLUE)]))
    } else {
        (sp("◦", DIM), line([sp("idle", DIM)]))
    };
    let mut tail = Line::new();
    let push = |t: &mut Line, s: Span| {
        if !t.spans.is_empty() {
            t.spans.push(plain("  "));
        }
        t.spans.push(s);
    };
    let (mut planned, mut ticked) = (0i64, 0i64);
    if let Some(ev) = m
        .kept
        .peek::<WorkState>()
        .and_then(|st| st.views.get(&a.key))
    {
        for e in ev.iter().filter(|e| e.run.is_empty()) {
            match e.kind {
                EventKind::Plan => planned += e.n,
                EventKind::Tick => ticked += 1,
                _ => {}
            }
        }
    }
    if planned > 0 {
        push(
            &mut tail,
            sp(format!("✓ {}/{planned}", ticked.min(planned)), GREEN),
        );
    }
    if a.spend.context > 0 {
        let win = m.reg.context_window(a.kind.as_str(), &a.spend.model);
        let pc = fill_of(a.spend.context, win, Default::default()).pct() as i64;
        let c = if pc >= 80 { YELLOW } else { DIM };
        push(&mut tail, sp(format!("ctx {pc}%"), c));
    }
    push(&mut tail, sp(right(&age(now - a.job.updated_at), 5), FAINT));
    let mut l = line([
        plain(" "),
        marker,
        plain(" "),
        spb(fit_str(&one_line(&a.display_name), 26), TEXT),
        plain(" "),
    ]);
    let room = w as isize - l.width() as isize - tail.width() as isize - 2;
    join(&mut l, fit(state, room.max(0) as usize));
    l.spans.push(plain("  "));
    join(&mut l, tail);
    l
}

/// The row picked, by its id at sel, following it if rows moved, else the
/// nearest row that can be picked to pos; rows are (id, pickable)
/// (pickRow).
pub fn pick_row(rows: &[(String, bool)], pos: &mut usize, sel: &mut String) -> Option<usize> {
    if let Some((id, ok)) = rows.get(*pos)
        && id == sel
        && *ok
    {
        return Some(*pos);
    }
    if let Some(i) = rows.iter().position(|(id, ok)| id == sel && *ok) {
        *pos = i;
        return Some(i);
    }
    for d in 0..rows.len() as isize {
        for i in [*pos as isize + d, *pos as isize - d] {
            if i >= 0 && (i as usize) < rows.len() && rows[i as usize].1 {
                *pos = i as usize;
                *sel = rows[*pos].0.clone();
                return Some(*pos);
            }
        }
    }
    None
}
