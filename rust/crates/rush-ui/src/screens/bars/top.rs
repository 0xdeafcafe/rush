//! The top bar: the header's right, about every agent at once. Its
//! segments (bars.go:49 topSegs), the plan as each provider's accounts in
//! the order rush moves through them (bars_quiet.go: quietPlan :22,
//! acctQueues :52, otherDrains :405, startPlan :418's account, headerGauges
//! :418, quietSystem :573), the providers in use with a meter each
//! (usagebar.go: inUseRows :24, otherMeters :95), the account in use in
//! detail (view.go: activeUsage :555, usageDetail :590, usageMeter :496,
//! runsOut :530; agentquota.go:206 upcoming) and the lines down the right
//! (view.go:256 topRights). The meters' drawing is list_extras' (plan.rs,
//! meters.rs); the accounts and their limits are the accounts group's.

use rush_core::text::{Line, Span};
use rush_core::time::Ms;
use rush_core::usage::{EVERY, LEAD, Quota, Window};

use super::BAR_TOP;
use super::line::{Ctx, Seg, bar_line};
use crate::header::Drops;
use crate::model::Model;
use crate::screens::accounts::look::{LOGINS_KIND, look_of, provider_name};
use crate::screens::accounts::rows::{
    AcctRow, account_rows, accounts_of, start_account, start_dir, start_kind, start_profile,
    start_quota, switches,
};
use crate::screens::accounts::state::{cfg, peek};
use crate::screens::list_extras::meters::{MeterRow, meter_row, mini_bar, pct, usage_color};
use crate::screens::list_extras::plan::{
    InQueue, Queue, account_chain, compact_window, drains, plan_rows, plan_summary, reset_in,
    roughly, system_alerts, upcoming,
};
use crate::screens::projects_machine::{TEMP_SHOWN, machine, temp_of};
use crate::screens::social::parts::disk;
use crate::style::*;

const HOUR: Ms = 3_600_000;

/// Now, as the readings are reckoned against (snap.At).
pub fn now(m: &Model) -> Ms {
    m.clock.unwrap_or_else(rush_core::time::now)
}

fn dim(s: impl Into<String>) -> Span {
    sp(s, DIM)
}

fn one(s: Span) -> Line {
    line([s])
}

/// The top bar's segments (topSegs).
pub static SEGS: &[Seg] = &[
    Seg {
        id: "plan",
        name: "Plan summary",
        about: "the default provider’s usage, with fixed window order; add Plan usage for all accounts, meters and reset times",
        draw: |x| quiet_plan(x.m, x.room),
    },
    Seg {
        id: "memory",
        name: "Agent memory",
        about: "memory held by agents and their processes, with a fixed-width reading",
        draw: |x| {
            let g = machine(x.m).total_mem as f64 / (1u64 << 30) as f64;
            one(dim(format!("{g:4.1}G RAM")))
        },
    },
    Seg {
        id: "system",
        name: "System alerts",
        about: "network trouble, low disk or low battery; quiet while healthy",
        draw: |_| quiet_system(),
    },
    Seg {
        id: "statushelp",
        name: "Status customization",
        about: "opens the status-line editor: full CPU, tokens, storage, battery and detailed usage remain available",
        draw: |_| one(sp("#statusline details", FAINT)),
    },
    Seg {
        id: "today",
        name: "Spend today",
        about: "what every account has spent today",
        draw: |x| line([sp(money(x.today), TEXT), dim(" today")]),
    },
    Seg {
        id: "usage",
        name: "Plan usage",
        about: "the account in use: its 5-hour and weekly limits, and when they reset; then every other provider's, the least room first",
        draw: |x| active_usage(x.m, x.room),
    },
    Seg {
        id: "accounts",
        name: "Every account",
        about: "each account's shortest limit, of every agent, when there are several",
        draw: |x| {
            let mut parts = Vec::new();
            for r in account_rows(x.m) {
                // The shortest window: the one that moves.
                if !r.head
                    && let Some(w) = r.q.windows.first()
                {
                    parts.push(line([
                        dim(format!("{} ", r.name(x.m))),
                        sp(format!("{:.0}%", w.percent), usage_color(w.percent)),
                    ]));
                }
            }
            if parts.len() < 2 {
                return Line::new();
            }
            let mut out = Line::new();
            for (i, p) in parts.into_iter().enumerate() {
                if i > 0 {
                    out.spans.push(plain("  "));
                }
                join(&mut out, p);
            }
            out
        },
    },
    Seg {
        id: "agents",
        name: "Agents",
        about: "how many agents there are, and how many are running",
        draw: |x| {
            let n = x.m.agents.len();
            let live = x.m.agents.iter().filter(|a| a.job.live()).count();
            if n == 0 {
                return Line::new();
            }
            one(dim(format!("{n} agents · {live} running")))
        },
    },
    Seg {
        id: "ram",
        name: "Memory",
        about: "what agents and their processes hold",
        draw: |x| one(dim(format!("{} ram", mem(machine(x.m).total_mem)))),
    },
    Seg {
        id: "tokens",
        name: "Tokens",
        about: "context tokens held by every agent still working",
        draw: |x| {
            let n: i64 =
                x.m.agents
                    .iter()
                    .filter(|a| a.job.live())
                    .map(|a| a.spend.context)
                    .sum();
            if n == 0 {
                return Line::new();
            }
            one(dim(format!("{} tokens", tokens(n))))
        },
    },
    Seg {
        id: "cpu",
        name: "CPU",
        about: "what agents and their processes use",
        draw: |x| one(dim(format!("{:.0}% cpu", machine(x.m).total_cpu))),
    },
    Seg {
        id: "net",
        name: "Network",
        about: "whether the network has been steady: yellow while the API has lately failed, been slow, or the network changed, offline in red while it can't be reached · #network says more",
        draw: |_| net_seg(),
    },
    Seg {
        id: "tmp",
        name: "Temp files",
        about: "agents' scratch left on disk, when there's much",
        draw: |x| {
            let t: i64 = x.m.agents.iter().map(|a| temp_of(x.m, &a.key)).sum();
            if t >= TEMP_SHOWN {
                return one(dim(format!("{} tmp", disk(t))));
            }
            Line::new()
        },
    },
    Seg {
        id: "disk",
        name: "Free disk",
        about: "space left on the disk agents work on: yellow under 20G, red under 5G",
        draw: |_| {
            let (_, d) = rush_sys::sysinfo::now();
            if d.total == 0 {
                return Line::new();
            }
            let v = format!("{} free", disk(d.free as i64));
            one(if d.free < 5 << 30 {
                sp(v, RED)
            } else if d.free < 20 << 30 {
                sp(v, YELLOW)
            } else {
                dim(v)
            })
        },
    },
    Seg {
        id: "battery",
        name: "Battery",
        about: "the battery's charge, ⚡ while plugged in: yellow under 30%, red under 15% off power",
        draw: |_| {
            let (b, _) = rush_sys::sysinfo::now();
            if !b.present {
                return Line::new();
            }
            let v = format!("{}% batt", b.percent);
            one(if b.charging {
                dim(format!("⚡{v}"))
            } else if b.percent < 15 {
                sp(v, RED)
            } else if b.percent < 30 {
                sp(v, YELLOW)
            } else {
                dim(v)
            })
        },
    },
    Seg {
        id: "account",
        name: "Account",
        about: "the agent and account new agents start on, and their profile",
        draw: |x| crate::screens::accounts::look::start_tag(x.m),
    },
    Seg {
        id: "clock",
        name: "Clock",
        about: "the time of day",
        draw: |x| one(dim(crate::screens::accounts::rows::clock(now(x.m), false))),
    },
    Seg {
        id: "version",
        name: "rush version",
        about: "the rush this window runs: yellow once a newer one is installed (#reload), orange while a newer one is out (#update)",
        draw: |x| x.m.version_tag(),
    },
];

/// The network: whether it's been steady. Red while the API can't be
/// reached, yellow while it's recently failed, been slow, or changed
/// (netsheet.go:65 netSeg).
fn net_seg() -> Line {
    let s = rush_net::netwatch::now();
    one(if !s.known {
        return Line::new();
    } else if !s.up {
        sp("offline", RED)
    } else if !s.steady {
        sp("net shaky", YELLOW)
    } else {
        dim("net steady")
    })
}

/// Only what needs attention: healthy battery, disk and network say
/// nothing (quietSystem).
pub fn quiet_system() -> Line {
    let (b, d) = rush_sys::sysinfo::now();
    let n = rush_net::netwatch::now();
    system_alerts(
        b.present.then_some((b.percent as f64, b.charging)),
        Some((d.free, d.total)),
        n.known.then_some((n.up, n.steady)),
    )
}

/// The agents' leftovers among the counts (view.go:189): how many, what
/// they hold, and where to end them.
pub fn orphans(m: &Model) -> Option<Line> {
    let mc = machine(m);
    if mc.orphans == 0 {
        return None;
    }
    Some(line([
        sp(
            format!("{} orphaned · {}", mc.orphans, mem(mc.orphan_mem)),
            YELLOW,
        ),
        dim(" · Projects to end"),
    ]))
}

// ---- the accounts ----

/// An account row as a meter draws it.
pub fn meter_of(m: &Model, r: &AcctRow) -> MeterRow {
    let l = look_of(&m.reg, &r.kind);
    MeterRow {
        glyph: l.glyph.to_string(),
        colour: Some(l.colour()),
        label: r.label(m),
        q: r.q.clone(),
        kind: r.kind.clone(),
        name: r.name(m),
        current: r.current,
    }
}

/// Each provider's account in use, or its one sign-in (inUseRows).
pub fn in_use_rows(m: &Model) -> Vec<MeterRow> {
    let rows = account_rows(m);
    let mut out: Vec<MeterRow> = rows
        .iter()
        .filter(|r| !r.head && r.current)
        .map(|r| meter_of(m, r))
        .collect();
    let current = out.len();
    for r in rows.iter().filter(|r| r.head) {
        // No account of it kept, or none in use: its own line.
        if !out[..current].iter().any(|c| c.kind == r.kind) {
            out.push(meter_of(m, r));
        }
    }
    out
}

/// The providers with more than one account to move between, the one new
/// sessions start on first, then the default profile's order; and every
/// other provider's account in use, or its one sign-in, that has limits
/// to show (acctQueues).
pub fn acct_queues(m: &Model) -> (Vec<Queue>, Vec<MeterRow>) {
    let now = now(m);
    let start = start_kind(m);
    let rows = account_rows(m);
    let mut queues: Vec<Queue> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if !r.head {
            continue;
        }
        let l = look_of(&m.reg, &r.kind);
        let mut qu = Queue {
            kind: r.kind.clone(),
            glyph: l.glyph.to_string(),
            colour: Some(l.colour()),
            accts: Vec::new(),
        };
        if r.kind == LOGINS_KIND {
            // Claude Code's logins, in the order autoSwitch moves through
            // them.
            let logins: Vec<rush_fleet::LoginView> = peek(m)
                .logins
                .iter()
                .map(|l| rush_fleet::LoginView {
                    login: l.login.clone(),
                    usage: l.usage.clone(),
                    quota: l.quota.clone(),
                    current: l.current,
                })
                .collect();
            for k in rush_fleet::chain(&logins, now) {
                qu.accts.push(InQueue {
                    name: k.login.login.name.clone(),
                    cur: k.login.current,
                    q: k.login.quota.clone(),
                });
            }
        } else if switches(m, &r.kind) {
            let accts: Vec<MeterRow> = accounts_of(&rows[i + 1..], &r.kind)
                .iter()
                .map(|a| meter_of(m, a))
                .collect();
            for a in account_chain(&accts) {
                qu.accts.push(InQueue {
                    name: a.name,
                    cur: a.current,
                    q: a.q,
                });
            }
        }
        if qu.accts.len() < 2 {
            continue;
        }
        if r.kind == start {
            queues.insert(0, qu);
        } else {
            queues.push(qu);
        }
    }
    let single = in_use_rows(m)
        .into_iter()
        .filter(|r| !r.q.windows.is_empty() && !queues.iter().any(|q| q.kind == r.kind))
        .collect();
    (queues, single)
}

/// The provider and account new agents start on, and its limits
/// (startPlan).
pub fn start_plan(m: &Model) -> (String, Quota) {
    let k = start_kind(m);
    let mut q = start_quota(m);
    let mut account = account_rows(m)
        .into_iter()
        .find(|r| r.kind == k && r.current && !r.head)
        .map(|r| r.name(m))
        .unwrap_or_default();
    if q.is_none() {
        // Claude Code: the login sessions run as.
        if let Some(l) = peek(m).logins.iter().find(|l| l.current) {
            q = Some(l.quota.clone());
            account = l.name.clone();
        }
    }
    let mut name = provider_name(&m.reg, &m.reg.provider_of(&k));
    if !account.is_empty() {
        name += " · ";
        name += &rush_draw::truncate(&account, 12, "…");
    }
    (name, q.unwrap_or_default())
}

/// The default provider's plan, fixed in its order; where a provider has
/// more than one account, its accounts in the order rush moves sessions
/// through them, fitted to w (quietPlan).
pub fn quiet_plan(m: &Model, w: usize) -> Line {
    let now = now(m);
    let (queues, single) = acct_queues(m);
    if !queues.is_empty() {
        return plan_rows(&queues, &single, now, &[w])
            .into_iter()
            .next()
            .unwrap_or_default();
    }
    let (name, q) = start_plan(m);
    let k = start_kind(m);
    let l = look_of(&m.reg, &k);
    let mut who = line([sp(l.glyph, l.colour())]);
    if let Some((_, account)) = name.split_once(" · ") {
        who.spans.push(dim(format!(" {account}")));
    }
    let mut out = plan_summary(who, &q, now);
    join(&mut out, other_drains(m, &k));
    out
}

/// Every other provider in use after k: its glyph and how much of its
/// tightest window is used (otherDrains).
fn other_drains(m: &Model, k: &str) -> Line {
    let rows: Vec<MeterRow> = in_use_rows(m)
        .into_iter()
        .filter(|r| r.kind != k && !r.q.windows.is_empty())
        .collect();
    drains(&rows, now(m))
}

/// The providers other than k, a mini meter each (otherMeters).
fn other_meters(m: &Model, k: &str, w: usize) -> Line {
    let now = now(m);
    let rows: Vec<MeterRow> = in_use_rows(m)
        .into_iter()
        .filter(|r| r.kind != k)
        .map(|mut r| {
            r.q = r.q.since(now); // a window that has reset since is empty
            r
        })
        .collect();
    meter_row(&rows, w)
}

fn trim_lead(mut l: Line) -> Line {
    while let Some(first) = l.spans.first_mut() {
        let t = first.text.trim_start_matches(' ');
        if t.is_empty() {
            l.spans.remove(0);
            continue;
        }
        first.text = t.to_string();
        break;
    }
    l
}

/// The account in use's plan usage, with when each window resets, quiet
/// unless it is high, and near a switch the account rush moves on to next;
/// then every other provider's, a mini meter each. Short of room in w,
/// the detail halves its bars, then drops its reset times, then goes,
/// before another provider's meter is left out (activeUsage).
pub fn active_usage(m: &Model, w: usize) -> Line {
    let mut u = peek(m)
        .logins
        .iter()
        .find(|l| l.current)
        .map(|l| l.quota.clone())
        .unwrap_or_default();
    if let Some(q) = start_quota(m) {
        u = q; // new sessions run another agent: its account's
    }
    let k = start_kind(m);
    if u.windows.is_empty() {
        return trim_lead(other_meters(m, &k, w));
    }
    let mut others = other_meters(m, &k, 1 << 16);
    if others.width() > 0 {
        let mut l = line([sp("  │", FAINT)]);
        join(&mut l, others);
        others = l;
    }
    for (cells, when) in [(10, true), (5, true), (5, false)] {
        let mut s = usage_detail(m, &u, cells, when);
        join(&mut s, others.clone());
        if s.width() <= w {
            return s;
        }
    }
    trim_lead(meter_row(&in_use_rows(m), w))
}

/// Whose usage the header shows: the provider's glyph, the account new
/// sessions start on, and their profile, always named (usageTag).
fn usage_tag(m: &Model) -> Line {
    let k = start_kind(m);
    let l = look_of(&m.reg, &k);
    let mut s = line([
        sp(l.glyph, l.colour()),
        plain(" "),
        sp(start_account(m), TEXT),
    ]);
    let p = start_profile(m, &start_dir(m)).name;
    if !p.is_empty() {
        s.spans.push(sp(" · ", FAINT));
        s.spans.push(dim(p));
    }
    s
}

/// activeUsage's own account, its meters cells wide (usageDetail).
fn usage_detail(m: &Model, u: &Quota, cells: usize, when: bool) -> Line {
    let now = now(m);
    let mut parts: Vec<Line> = u
        .windows
        .iter()
        .map(|win| usage_meter(win, win.rate(now), now, cells, when))
        .collect();
    let k = start_kind(m);
    let accts: Vec<MeterRow> = accounts_of(&account_rows(m), &k)
        .iter()
        .map(|r| meter_of(m, r))
        .collect();
    let waits = cfg(m).default_profile().limit() == rush_state::profiles::LIMIT_WAIT;
    if let Some(next) = upcoming(&accts, u.used(""), waits, now) {
        // Where sessions go at the switch: glyph, name, and a short bar
        // of its tightest window, in the meters' own style.
        let n = next.q.used("");
        let mut l = line([
            sp(
                format!("↪ at {} ", pct(u.switch_point("", LEAD, now))),
                FAINT,
            ),
            sp(next.glyph.clone(), next.colour.unwrap_or(TEXT)),
            plain(" "),
            sp(next.name.clone(), TEXT),
            plain(" "),
        ]);
        join(&mut l, mini_bar(n));
        l.spans.push(plain(" "));
        l.spans.push(sp(pct(n), usage_color(n)));
        parts.push(l);
    }
    let mut s = usage_tag(m);
    s.spans.push(plain("  "));
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            s.spans.push(plain("   "));
        }
        join(&mut s, p);
    }
    let every = EVERY.as_millis() as Ms;
    if u.fetched_at != 0 && now - u.fetched_at > 3 * every {
        let day = now - u.fetched_at > 20 * HOUR;
        let when = crate::screens::accounts::rows::clock(u.fetched_at, day);
        s.spans.push(sp(format!(" as of {when}"), FAINT));
    }
    s
}

/// One plan window: a bar of cells of what's used, coloured by how it is
/// going, with a tick for how far through the window we are, so being
/// ahead of pace shows at a glance; then the percentage and, with when,
/// when it resets (usageMeter).
pub fn usage_meter(win: &Window, rate: f64, now: Ms, w: usize, when: bool) -> Line {
    let pct_ = win.percent;
    let span = win.span.as_millis() as Ms;
    let resets = win.resets_at;
    // The share of the window gone, when we know when it ends.
    let pace = (resets != 0 && resets > now && span > 0)
        .then(|| 1.0 - (resets - now) as f64 / span as f64);
    let c = if pct_ >= 80.0 {
        RED
    } else if pct_ >= 50.0 || pace.is_some_and(|p| pct_ / 100.0 > p + 0.1) {
        YELLOW // high, or burning faster than the window allows
    } else {
        GREEN
    };
    let mut fill = ((pct_ / 100.0 * w as f64 + 0.5).max(0.0) as usize).min(w);
    if pct_ > 0.0 && fill == 0 {
        fill = 1;
    }
    let tick = pace.map(|p| ((p * w as f64).max(0.0) as usize).min(w.saturating_sub(1)));
    let mut s = line([dim(format!("{} ", win.label))]);
    for i in 0..w {
        let at = tick == Some(i);
        s.spans.push(match (at, i < fill) {
            (true, true) => sp("╋", TEXT),
            (true, false) => sp("┼", SUB),
            (false, true) => sp("━", c),
            (false, false) => sp("─", FAINT),
        });
    }
    s.spans.push(plain(" "));
    s.spans.push(sp(format!("{pct_:.0}%"), c));
    if when {
        if let Some(r) = runs_out(pct_, rate, resets, now) {
            s.spans.push(r);
        }
        if let Some(r) = reset_in(resets, now) {
            s.spans.push(r);
        }
    }
    s
}

/// How long the window lasts at the rate it fills: red when that is
/// before it resets, "lasts" when it outlasts the reset, nothing until a
/// rate is known (runsOut).
fn runs_out(pct_: f64, rate: f64, resets: Ms, now: Ms) -> Option<Span> {
    if rate <= 0.0 || pct_ >= 100.0 {
        return None;
    }
    let left = ((100.0 - pct_) / rate * HOUR as f64) as Ms;
    if resets != 0 && resets > now && left >= resets - now {
        return Some(sp(" lasts", FAINT));
    }
    Some(sp(format!(" out in {}", roughly(left)), RED))
}

/// The header's right side while the top bar is the default: spend and
/// the account, a bar for each limit, then the machine, a row each. None
/// when they don't fit in w (headerGauges).
pub fn header_gauges(m: &Model, today: f64, w: usize) -> Option<Vec<Line>> {
    if super::layout(m, BAR_TOP).lines != rush_bundled::statusline::default_top().lines {
        return None; // a top bar of your own is drawn as you built it
    }
    let (name, q) = start_plan(m);
    let now = now(m);
    let mut rows = vec![line([
        sp(money(today), TEXT),
        dim(format!(" today · {name}")),
    ])];
    let mut wins = q.windows.clone();
    wins.sort_by(|a, b| match (a.span.is_zero(), b.span.is_zero()) {
        _ if a.span == b.span => a.label.cmp(&b.label),
        (true, _) => std::cmp::Ordering::Greater,
        (_, true) => std::cmp::Ordering::Less,
        _ => a.span.cmp(&b.span),
    });
    for win in wins.iter().take(2) {
        rows.push(compact_window(win, now));
    }
    if wins.is_empty() && !q.problem.is_empty() {
        rows.push(line([dim("limits "), sp("unavailable", YELLOW)]));
    }
    let g = machine(m).total_mem as f64 / (1u64 << 30) as f64;
    let mut mc = line([dim(format!("{g:.1}G RAM · {} agents", m.agents.len()))]);
    let s = quiet_system();
    if s.width() > 0 {
        mc.spans.push(dim(" · "));
        join(&mut mc, s);
    }
    rows.push(mc);
    if rows.iter().any(|r| r.width() > w) {
        return None;
    }
    Some(rows)
}

/// The top bar's lines, one in each of rooms' rows. The plan alone on a
/// line takes the rows the lines after it leave spare, for a line each of
/// the providers with accounts to move between (topRights).
pub fn top_rights(m: &Model, rooms: &[usize], drops: &mut Drops) -> Vec<Line> {
    let lines = super::layout(m, BAR_TOP).lines;
    let mut out = vec![Line::new(); rooms.len()];
    let mut x = Ctx::new(m);
    let mut row = 0;
    for (i, l) in lines.iter().enumerate() {
        if row >= rooms.len() {
            break;
        }
        let spare = rooms.len() as isize - row as isize - (lines.len() - i - 1) as isize;
        if l.len() == 1 && l[0] == "plan" && spare > 1 {
            let (queues, single) = acct_queues(m);
            if !queues.is_empty() {
                let got = plan_rows(&queues, &single, now(m), &rooms[row..row + spare as usize]);
                for g in got {
                    out[row] = g;
                    row += 1;
                }
                drops.push((BAR_TOP * 10 + i, Vec::new()));
                continue;
            }
        }
        out[row] = bar_line(m, BAR_TOP, i, &mut x, rooms[row], drops);
        row += 1;
    }
    out
}
