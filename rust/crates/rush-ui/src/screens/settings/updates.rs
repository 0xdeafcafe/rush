//! Settings › Updates: rush and each harness it runs, what's installed and
//! what's newest, and enter updates the one under the cursor. After Go's
//! internal/ui/settings_updates.go (updatesState :27, updatesCount :46,
//! checkHarnesses :61, recheckAfter :99, updateHarness :107, updatesPage
//! :215, updatesSections :219, updateKey :238, updateLine :253, rushRow
//! :265, harnessRow :310, hostsRow :337, goneNote :201, watchHosts
//! :143-200 as of c075468d, startStamp :45), reload.go:134 (watchBinary)
//! and internal/harnessup's Check :76 (which harnesses, and where their
//! programs really are).
//!
//! Installing a newer rush and reloading onto it are the commands #update
//! and #reload, run from here when they're registered. The sessions' hosts
//! on an older rush are counted with each reading of the list, every 30s
//! at most, off the UI; enter on Sessions restarts the idle ones, and the
//! rush that leads restarts them by itself only with Restart on install,
//! or while a reload still has sessions to move.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use rush_host::client::Lister;
use rush_host::wire::BinStamp;

use rush_core::text::Line;
use rush_harnessup::{Harness, State as Up, Status};
use rush_update::Release;

use super::common::reg;
use super::form::{Section, Setting};
use super::paint::{first_non_empty, trunc};
use super::{Page, State};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// What the page knows, all of it read off the UI (updatesState).
#[derive(Default)]
pub struct Updates {
    pub rows: Vec<Status>,
    pub checked: bool,
    pub checking: bool,
    /// Harnesses being updated, by kind.
    pub running: HashSet<String>,
    /// The newest rush, whether or not it's newer.
    pub rush_new: Release,
    /// The newest rush when it's newer than this one.
    pub newer: Release,
    /// The count was flashed once.
    pub said: bool,
    /// Live sessions on an older rush than the installed one.
    pub stale: usize,
    /// Restarts the last enter asked for that failed.
    pub failed: usize,
    /// Sessions on an older rush whose folder is gone.
    pub gone: usize,
    pub checked_at: Option<Instant>,
    /// A look at the hosts is out, and the installed rush as the last one
    /// found it.
    pub sweeping: bool,
    pub installed: BinStamp,
    /// A newer rush was installed since this one started, and the news
    /// said so.
    pub rebuilt: bool,
    pub rebuilt_said: bool,
    /// Lists the sessions again and again, reading only what changed.
    pub lister: Arc<Mutex<Lister>>,
    /// Stands in for the sweep and the installed rush's stamp, in tests.
    pub sweep: Option<SweepFn>,
}

/// Counts the sessions on an older rush than installed, restarting the
/// idle ones when told to: (stale, restarted, failed, gone)
/// (Lister.Sweep).
pub type SweepFn = fn(&mut Lister, BinStamp, bool) -> (usize, usize, usize, usize);

/// The rush binary as it was when this one started (startStamp).
static START_STAMP: LazyLock<BinStamp> = LazyLock::new(BinStamp::exe);

/// Notes the running rush's stamp, as it starts.
pub fn note_start() {
    LazyLock::force(&START_STAMP);
}

/// Every 30 seconds of the tick (chores.rs), while a loop runs: the
/// installed rush looked at, and the sessions on an older one counted;
/// the rush that leads restarts the idle ones with Restart on install
/// (reload.go:134 watchBinary).
pub fn watch_binary(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    watch_hosts(m, true, false);
}

/// The installed rush's binary looked at, and the sessions on an older one
/// counted; with restart, the ones between turns are restarted on it.
/// That's you asking (enter, asked), or Restart on install once the binary
/// has stopped changing, in the rush that leads; without either a new
/// binary alone restarts nothing. A reload asks too: the leading rush
/// restarts sessions until every one is on the rush it reloaded to. Off
/// the UI (watchHosts).
pub fn watch_hosts(m: &mut Model, restart: bool, asked: bool) {
    let lead = m.leads();
    let u = &mut m.kept::<State>().updates;
    if u.sweeping {
        return;
    }
    u.sweeping = true;
    let was = u.installed;
    let lister = u.lister.clone();
    let sweep = u.sweep.unwrap_or(|l, now, r| l.sweep(now, r));
    let fake = u.sweep.is_some();
    m.later(
        move || {
            // In tests the stamp is the start's: nothing was installed.
            let now = if fake { *START_STAMP } else { BinStamp::exe() };
            // Not asked, it restarts only by Restart on install, and only
            // in the rush that leads.
            let auto = !asked
                && restart
                && lead
                && (fake || rush_state::Store::load().config.restart_on_install);
            // The same stamp as the last watch: a rush still being written
            // is never restarted onto.
            let settled = now == was;
            let mut restart = (asked && restart) || auto && settled;
            // The reload's note names the rush it wants: settled enough.
            let reloading =
                lead && !now.mod_.is_zero() && !fake && rush_host::reload::reload_wanted(now);
            restart = restart || reloading;
            let (mut stale, mut restarted, mut failed, mut gone) = (0, 0, 0, 0);
            if !now.mod_.is_zero() {
                let mut l = lister.lock().unwrap_or_else(|e| e.into_inner());
                (stale, restarted, failed, gone) = sweep(&mut l, now, restart);
            }
            if reloading && stale <= failed {
                rush_host::reload::reload_done();
            }
            let restart_on_install = auto;
            Swept {
                now,
                settled,
                restart,
                restart_on_install,
                stale,
                restarted,
                failed,
                gone,
            }
        },
        move |m, s| on_swept(m, s, asked),
    );
}

/// What a look at the hosts found.
pub struct Swept {
    now: BinStamp,
    settled: bool,
    restart: bool,
    restart_on_install: bool,
    stale: usize,
    restarted: usize,
    failed: usize,
    gone: usize,
}

/// A look at the hosts taken in: the count kept, a new install noticed
/// once, restarts said (an answer when asked, else news), and with
/// Restart on install the reload onto the new rush (watchHosts' apply).
fn on_swept(m: &mut Model, s: Swept, asked: bool) -> Out {
    let u = &mut m.kept::<State>().updates;
    if s.stale != u.stale || asked || s.restarted > 0 || s.failed > 0 {
        rush_sys::events::log(
            "stale-hosts",
            serde_json::json!({
                "stale": s.stale, "was": u.stale, "restarted": s.restarted,
                "failed": s.failed, "gone": s.gone, "restart": s.restart, "asked": asked,
            }),
        );
    }
    if s.restart {
        u.failed = s.failed;
    }
    u.sweeping = false;
    u.stale = s.stale;
    u.gone = s.gone;
    u.installed = s.now;
    let mut news = None;
    if !s.now.mod_.is_zero() && s.now.mod_.after(START_STAMP.mod_) && !u.rebuilt {
        rush_sys::events::log(
            "rush-rebuilt",
            serde_json::json!({"installedAt": s.now.mod_, "startedAt": START_STAMP.mod_}),
        );
        u.rebuilt = true;
        if !u.rebuilt_said {
            u.rebuilt_said = true;
            news = Some("rush was installed again: #reload runs it, sessions carry on");
        }
    }
    let rebuilt = u.rebuilt;
    if rebuilt {
        m.kept::<crate::screens::social::services::Update>().rebuilt = true;
    }
    if let Some(n) = news {
        m.news(n, "");
    }
    let said = if s.failed > 0 {
        Some(format!(
            "restarted {} sessions on the installed rush: {} failed, still on the old one · {} still busy{}",
            s.restarted,
            s.failed,
            s.stale - s.failed,
            gone_note(s.gone)
        ))
    } else if asked || s.restarted > 0 {
        Some(format!(
            "restarted {} sessions on the installed rush: {} still busy{}",
            s.restarted,
            s.stale,
            gone_note(s.gone)
        ))
    } else {
        None
    };
    // Asked for, it's an answer; on its own, it's news.
    if let Some(t) = said {
        match (asked, s.failed > 0) {
            (true, true) => m.flash_err(&t),
            (true, false) => m.flash(&t),
            (false, _) => m.news(&t, ""),
        }
    }
    if rebuilt
        && s.settled
        && s.restart_on_install
        && !rush_core::dryrun::would(
            "reload",
            "this rush",
            "restart on install: a new rush was installed",
        )
    {
        rush_sys::events::log("reload", serde_json::json!({"why": "restart on install"}));
        return m.reload(false);
    }
    Out::Draw
}

fn upd(m: &Model) -> Option<&Updates> {
    m.kept.peek::<State>().map(|s| &s.updates)
}

/// The page.
pub fn page() -> Page {
    Page {
        name: "Updates",
        form: Some(sections),
        keys: vec!["r".into(), "check again".into()],
        ..Page::default()
    }
}

/// How many things have an update: the harnesses, and rush
/// (updatesCount).
pub fn count(m: &Model) -> usize {
    let Some(u) = upd(m) else {
        return 0;
    };
    u.rows.iter().filter(|r| r.state() == Up::Available).count()
        + usize::from(!u.newer.version.is_empty())
}

/// Where rush's stand-ins for agents' programs are (host.ShimDir).
fn shim_dir() -> PathBuf {
    rush_state::cache_path("shims")
}

/// A program on PATH, as rush finds agents.
fn find(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return Some(PathBuf::from(name));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| {
        use std::os::unix::fs::PermissionsExt;
        let c = d.join(name);
        let m = std::fs::metadata(&c).ok()?;
        (!m.is_dir() && m.permissions().mode() & 0o111 != 0).then_some(c)
    })
}

/// Every installed harness, once, by its real program (Check's list).
fn harnesses() -> Vec<Harness> {
    let r = reg();
    let shims = shim_dir();
    let without: std::ffi::OsString = std::env::var_os("PATH")
        .map(|p| {
            let keep: Vec<PathBuf> = std::env::split_paths(&p)
                .filter(|d| !d.as_os_str().is_empty() && *d != shims)
                .collect();
            std::env::join_paths(keep).unwrap_or_default()
        })
        .unwrap_or_default();
    let mut out: Vec<Harness> = Vec::new();
    for a in r.installed_all() {
        let k = a.kind();
        if r.current_kind(k.as_str()) != k {
            continue;
        }
        let program = r.program_of(k.as_str());
        let found = r.path(k.as_str());
        if program.is_empty() || found.is_empty() {
            continue;
        }
        let Some(path) =
            rush_harnessup::real_program(std::path::Path::new(&found), &program, &shims, &without)
        else {
            continue;
        };
        if out.iter().any(|h| h.path == path) {
            continue;
        }
        out.push(Harness {
            kind: k.0.clone(),
            name: a.name(),
            path,
            published: a
                .publisher()
                .map(|p| {
                    let p = p.published();
                    rush_harnessup::Published {
                        npm: p.npm,
                        pypi: p.pypi,
                        update: p.update,
                    }
                })
                .unwrap_or_default(),
        });
    }
    out
}

/// Looks for newer harnesses and rush, in the background; force asks the
/// registries again rather than trusting what was kept (checkHarnesses).
pub fn check(m: &mut Model, force: bool) {
    let u = &mut m.kept::<State>().updates;
    if u.checking {
        return;
    }
    u.checking = true;
    m.later(
        move || {
            let hs = harnesses();
            let rows = rush_harnessup::check(&rush_state::dir(), &hs, force, &|n| find(n));
            let (latest, newer) = rush_update::check(&rush_update::Quiet);
            let mut avail: Vec<String> = rows
                .iter()
                .filter(|r| r.state() == Up::Available)
                .map(|r| format!("{} {} → {}", r.kind, r.installed, r.latest))
                .collect();
            if newer {
                avail.push(format!("rush → {}", latest.version));
            }
            if !avail.is_empty() {
                let f = serde_json::json!({"updates": avail, "force": force});
                rush_events::log(
                    "updates-available",
                    f.as_object().cloned().unwrap_or_default(),
                );
            }
            (rows, latest, newer)
        },
        move |m, (rows, latest, newer)| {
            let u = &mut m.kept::<State>().updates;
            u.rows = rows;
            u.checked = true;
            u.checking = false;
            u.checked_at = Some(Instant::now());
            if newer && u.newer.version.is_empty() {
                u.newer = latest.clone();
            }
            u.rush_new = latest;
            let n = count(m);
            let u = &mut m.kept::<State>().updates;
            if n > 0 && !u.said {
                u.said = true;
                if force {
                    m.flash(&format!("{n} updates · Settings › Updates"));
                } else {
                    m.news(
                        &format!("{n} updates found: Settings › Updates has them"),
                        "",
                    );
                }
            }
            Out::Draw
        },
    );
}

/// check again once EVERY has passed (recheckAfter).
pub fn recheck_after(m: &mut Model) {
    let fresh = upd(m)
        .and_then(|u| u.checked_at)
        .is_some_and(|at| at.elapsed() < rush_harnessup::EVERY);
    if !fresh {
        check(m, false);
    }
}

/// Runs the update of one harness, then looks again (updateHarness).
fn update_harness(m: &mut Model, r: Status) -> Out {
    let u = &mut m.kept::<State>().updates;
    if u.running.contains(&r.kind) {
        m.flash(&format!("already updating {}…", r.name));
        return Out::Draw;
    }
    u.running.insert(r.kind.clone());
    m.flash(&format!("updating {}: {}…", r.name, r.update.label));
    let r2 = r.clone();
    m.later(
        move || {
            let err = rush_harnessup::run(&r2, &|n| find(n)).err();
            reg().recheck();
            let rows = rush_harnessup::check(&rush_state::dir(), &harnesses(), false, &|n| find(n));
            (err, rows)
        },
        move |m, (err, rows)| {
            let u = &mut m.kept::<State>().updates;
            u.running.remove(&r.kind);
            u.rows = rows;
            match err {
                Some(e) => m.fail(&e),
                None => m.flash(&format!("{} updated", r.name)),
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// Runs the command registered as name, as typing #name would; a flash
/// when none is.
fn run_command(m: &mut Model, name: &str) -> Out {
    match m
        .ext
        .commands
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.run)
    {
        Some(run) => run(m, ""),
        None => {
            m.flash_err(&format!("#{name} isn't in this rush yet"));
            Out::Draw
        }
    }
}

/// The page's sections: rush, the harnesses, the sessions' hosts
/// (updatesSections).
fn sections(m: &Model) -> Vec<Section> {
    let rush = Section::new("rush", vec![rush_row(m)]);
    let mut hs = Section {
        title: "Harnesses".into(),
        note: "installed → latest".into(),
        ..Section::default()
    };
    let (checked, rows) = upd(m).map_or((false, Vec::new()), |u| (u.checked, u.rows.clone()));
    if !checked {
        hs.rows.push(Setting {
            label: "checking".into(),
            line: Some(Rc::new(|_| line([sp("checking what's installed…", DIM)]))),
            ..Setting::default()
        });
    } else if rows.is_empty() {
        hs.rows.push(Setting {
            label: "none".into(),
            line: Some(Rc::new(|_| line([sp("No harness is installed.", DIM)]))),
            ..Setting::default()
        });
    }
    for r in rows {
        hs.rows.push(harness_row(m, r));
    }
    let hosts = Section {
        title: "Sessions".into(),
        note: "each runs in a host that stays on the rush it started with".into(),
        rows: vec![hosts_row(m)],
        ..Section::default()
    };
    vec![rush, hs, hosts]
}

/// A row's keys: enter does what the row does, r looks again (updateKey).
fn update_key(act: impl Fn(&mut Model) -> Out + 'static) -> super::form::KeyFn {
    Rc::new(move |m, s| match s {
        "enter" => act(m),
        "r" => {
            m.flash("checking again…");
            check(m, true);
            Out::Draw
        }
        _ => Out::Pass,
    })
}

/// One row: name, installed, newest and how it stands (updateLine).
pub fn update_line(w: usize, name: &str, have: &str, latest: &str, state: Line) -> Line {
    // Dates remain in About; keep the version and status readable in the row.
    let have = have.split(" (").next().unwrap_or("");
    let latest = latest.split(" (").next().unwrap_or("");
    let name_w = 24.min(12.max(w / 4));
    let version_w = 18.min(8.max((w as isize - name_w as isize - 24) / 2) as usize);
    let mut l = line([
        plain(fit_str(name, name_w)),
        plain(" "),
        plain(fit_str(have, version_w)),
        plain(" → "),
        plain(fit_str(latest, version_w)),
        plain(" "),
    ]);
    join(&mut l, state);
    trunc(l, w)
}

/// rush's own row (rushRow).
fn rush_row(m: &Model) -> Setting {
    let have = rush_update::current().map_or_else(|| "dev build".to_string(), |c| format!("v{c}"));
    let (rush_new, newer) = upd(m).map_or((Release::default(), Release::default()), |u| {
        (u.rush_new.clone(), u.newer.clone())
    });
    let mut latest = if rush_new.version.is_empty() {
        "unknown".to_string()
    } else {
        rush_new.short()
    };
    let state = if !newer.version.is_empty() {
        latest = newer.short();
        line([sp("update available", YELLOW)])
    } else if have == "dev build" || rush_new.version.is_empty() {
        line([sp("unknown", DIM)])
    } else {
        line([sp("up to date", GREEN)])
    };
    let (h1, l1, s1) = (have.clone(), latest.clone(), state.clone());
    let has_newer = !newer.version.is_empty();
    Setting {
        label: "rush".into(),
        line: Some(Rc::new(move |w| {
            update_line(w, "rush", &h1, &l1, s1.clone())
        })),
        about: Some(Rc::new(move || {
            (
                "rush".into(),
                format!(
                    "rush updates from its GitHub release, or with brew when brew installed it. Enter installs the newest (#update), then reloads it (#reload): your sessions carry on. Installed: {have}; latest: {latest}."
                ),
                state.plain(),
            )
        })),
        key: Some(update_key(move |m| {
            if has_newer {
                return run_command(m, "update");
            }
            m.flash("rush is up to date");
            Out::Draw
        })),
        keys: vec!["enter".into(), "update".into()],
        ..Setting::default()
    }
}

/// One harness's row (harnessRow).
fn harness_row(m: &Model, r: Status) -> Setting {
    let running = upd(m).is_some_and(|u| u.running.contains(&r.kind));
    let state = if running {
        line([sp("updating…", YELLOW)])
    } else {
        match r.state() {
            Up::Available => line([sp("update available", YELLOW)]),
            Up::UpToDate => line([sp("up to date", GREEN)]),
            Up::Unknown => line([sp("unknown", DIM)]),
        }
    };
    let name = reg().harness_label(&r.kind);
    let latest = first_non_empty(&[&r.latest, "unknown"]).to_string();
    let how = first_non_empty(&[
        &r.update.label,
        "unknown: update it the way you installed it",
    ])
    .to_string();
    let installed = first_non_empty(&[&r.installed, "?"]).to_string();
    let (n1, n2, n3, s1, s2, s3) = (
        name.clone(),
        name.clone(),
        name.clone(),
        state.clone(),
        state.clone(),
        state,
    );
    let path = tildify(&r.path.to_string_lossy());
    Setting {
        label: name,
        line: Some(Rc::new(move |w| {
            update_line(w, &n1, &installed, &latest, s1.clone())
        })),
        about: Some(Rc::new(move || {
            (
                n2.clone(),
                format!("Enter updates it with: {how}. It lives at {path}."),
                s2.plain(),
            )
        })),
        key: Some(update_key(move |m| {
            if r.state() != Up::Available {
                m.flash(&format!("{n3} is {}", s3.plain().to_lowercase()));
                return Out::Draw;
            }
            update_harness(m, r.clone())
        })),
        keys: vec!["enter".into(), "update".into()],
        ..Setting::default()
    }
}

/// The calm aside for sessions whose folder is gone: they can never move
/// to the installed rush, so they aren't a failure, just something to
/// close (goneNote).
pub fn gone_note(gone: usize) -> String {
    match gone {
        0 => String::new(),
        1 => " · 1 session's folder is gone · close it".into(),
        n => format!(" · {n} sessions' folders are gone · close them"),
    }
}

/// The sessions' hosts: how many are on an older rush (hostsRow).
pub(crate) fn hosts_row(m: &Model) -> Setting {
    let (n, f, g) = upd(m).map_or((0, 0, 0), |u| (u.stale, u.failed, u.gone));
    let mut what = "none".to_string();
    if n == 1 {
        what = "1 session on an older rush · enter restarts it once idle".into();
    } else if n > 0 {
        what = format!("{n} sessions on an older rush · enter restarts the idle ones");
    }
    if f > 0 {
        what.push_str(&format!(" · {f} failed to restart, so stayed as they were"));
    }
    what.push_str(&gone_note(g));
    let w1 = what.clone();
    Setting {
        label: "hosts".into(),
        line: Some(Rc::new(move |w| {
            trunc(
                line([
                    plain(fit_str("Sessions", 30)),
                    plain(" "),
                    plain(w1.clone()),
                ]),
                w,
            )
        })),
        about: Some(Rc::new(move || {
            (
                "Sessions".into(),
                "Enter restarts the idle ones on the installed rush now; one in a turn, with something queued or a question waiting, is left until it's over.".into(),
                what.clone(),
            )
        })),
        key: Some(update_key(|m| {
            watch_hosts(m, true, true);
            Out::Draw
        })),
        keys: vec!["enter".into(), "restart the idle ones".into()],
        ..Setting::default()
    }
}
