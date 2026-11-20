//! Settings › Disk: the disk engine's switch, its levels, every rule it
//! runs (on or off, its numbers per level, what it freed and left in the
//! last runs) and the rules proposed to it, waiting for your yes. With
//! them, what rush does when space runs short whatever the switch says
//! (`when_short`: offer a cleanup, the nearly-full clean, trimming a
//! running session's temp files), once General's Disk and memory (Go's
//! settings_general.go:13 had the offer; the rest are Rust-only).
//! Rust-only: Go had no disk engine. Laid out as remote.rs is (a page
//! backed by its own file, read off the UI), its rows' own keys as
//! ollama.rs's. The rules are rush-fleet's (disk/ruleset.rs: load_rules,
//! update_rules, built_in_rules); what the runs did is read back from the
//! disk-clean events disk/apply.rs logs (event_fields). Nothing here reads
//! or writes the disk while drawing.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rush_core::text::{Color, Line, width};
use rush_core::time::Ms;
use rush_fleet::disk::{
    KNOWN_TOOLS, Kind, LOW_UNDER, Level, NORMAL_UNDER, Params, Rule, RulesFile, built_in_rules,
    known_tool, load_rules, update_rules,
};
use serde_json::Value;

use super::form::{Section, Setting, choice, set};
use super::general::{off_pick, off_value};
use super::paint::on_off;
use super::paint::trunc;
use super::{Page, State, ask, config, config_mut};
use crate::frame::Out;
use crate::frame::confirm::Confirm;
use crate::frame::picker::{Pick, Picker};
use crate::model::Model;
use crate::style::*;

/// How long a reading of the rules and runs stays fresh.
const FRESH: Duration = Duration::from_secs(2);

/// How many of the newest disk-clean events the rows sum up.
pub const RUNS: usize = 20;

/// The event each engine run logs (rush-fleet's disk/apply.rs).
pub const EVENT: &str = "disk-clean";

/// The rules and runs as last read, off the UI.
#[derive(Clone, Debug, Default)]
pub struct Snap {
    pub at: Option<Instant>,
    pub reading: bool,
    /// The rules file, merged with the built-in rules; Err when it can't
    /// be read (nothing is changed then).
    pub file: Option<Result<RulesFile, String>>,
    pub runs: Runs,
    /// The row x was pressed on once: a second x removes it.
    pub removing: String,
}

/// What the newest runs did, summed up by rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Runs {
    /// How many runs were read.
    pub n: usize,
    /// The newest.
    pub last: Option<LastRun>,
    pub by_rule: BTreeMap<String, Tally>,
}

/// One run as its event says it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastRun {
    pub at: Ms,
    pub free: u64,
    /// "normal", "low", or "" when it didn't clean.
    pub level: String,
    pub freed: u64,
    /// Why it did nothing, when it didn't.
    pub why: String,
}

/// A rule's part in the runs read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// What its removals freed, summed.
    pub freed: u64,
    /// How many things it removed or ran.
    pub did: usize,
    /// How many things it would have removed that stayed, in the newest
    /// run that left any.
    pub left: usize,
    /// The first reason that run gave.
    pub why: String,
}

/// The page.
pub fn page() -> Page {
    Page {
        name: "Disk",
        form: Some(sections),
        prep: Some(want),
        ..Page::default()
    }
}

// ---- Research now: the cleanup agent's hook ----

/// What starts the cleanup agent in research mode: set by whoever owns
/// it (rush-ui-projects-machine's care.rs). Until it is, Research now
/// isn't shown.
static RESEARCH: OnceLock<fn(&mut Model) -> Out> = OnceLock::new();

/// Gives Research now its work: start the cleanup agent in research
/// mode, which writes proposals and removes nothing. Once per process.
pub fn on_research(f: fn(&mut Model) -> Out) {
    let _ = RESEARCH.set(f);
}

// ---- Durations and numbers ----

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;

/// A while as typed: a number and a unit (90s, 30m, 6h, 2d, 1w; the
/// words too: 30 minutes, 2 days), several added (1d12h), or 0. In
/// seconds.
pub fn parse_span(s: &str) -> Result<u64, String> {
    let t = s.trim().to_lowercase();
    let bad = || format!("{s:?} isn't a while: say 30m, 6h, 2d or 1w");
    if t.is_empty() {
        return Err(bad());
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return match t.parse::<u64>() {
            Ok(0) => Ok(0),
            _ => Err(bad()),
        };
    }
    let mut total: u64 = 0;
    let mut rest = t.as_str();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let n_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let n: f64 = rest[..n_end].parse().map_err(|_| bad())?;
        rest = rest[n_end..].trim_start();
        let u_end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let unit = match &rest[..u_end] {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => MINUTE,
            "h" | "hr" | "hrs" | "hour" | "hours" => HOUR,
            "d" | "day" | "days" => DAY,
            "w" | "wk" | "week" | "weeks" => WEEK,
            _ => return Err(bad()),
        };
        rest = rest[u_end..].trim_start_matches([',', ' ']);
        rest = rest.strip_prefix("and ").unwrap_or(rest);
        total = total.saturating_add((n * unit as f64).round() as u64);
    }
    Ok(total)
}

/// The largest unit secs is a whole number of, and its names.
fn unit_of(secs: u64) -> (u64, &'static str, &'static str, &'static str) {
    if secs.is_multiple_of(DAY) {
        (DAY, "d", "day", "days")
    } else if secs.is_multiple_of(HOUR) {
        (HOUR, "h", "hour", "hours")
    } else if secs.is_multiple_of(MINUTE) {
        (MINUTE, "m", "minute", "minutes")
    } else {
        (1, "s", "second", "seconds")
    }
}

/// A while in words: 2 days, 30 minutes, 1 hour, 36 hours; 0 for none.
pub fn span(secs: u64) -> String {
    if secs == 0 {
        return "0".into();
    }
    let (u, _, one, many) = unit_of(secs);
    let n = secs / u;
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A while in a few characters: 2d, 30m, 36h; 0 for none.
pub fn span_short(secs: u64) -> String {
    if secs == 0 {
        return "0".into();
    }
    let (u, s, _, _) = unit_of(secs);
    format!("{}{s}", secs / u)
}

/// One of a rule's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Num {
    /// How long nothing touched it before it goes.
    Idle,
    /// How old an artifact may get.
    Age,
    /// How many builds of a crate stay however old.
    Keep,
    /// How old a build past the newest keep may get.
    Superseded,
}

impl Num {
    pub fn name(self) -> &'static str {
        match self {
            Num::Idle => "idle",
            Num::Age => "age",
            Num::Keep => "keep",
            Num::Superseded => "superseded",
        }
    }

    fn of(s: &str) -> Option<Num> {
        [Num::Idle, Num::Age, Num::Keep, Num::Superseded]
            .into_iter()
            .find(|n| n.name() == s)
    }

    /// Its value in p.
    pub fn get(self, p: &Params) -> Option<u64> {
        match self {
            Num::Idle => p.idle_secs,
            Num::Age => p.age_secs,
            Num::Keep => p.keep.map(u64::from),
            Num::Superseded => p.superseded_secs,
        }
    }

    pub fn put(self, p: &mut Params, v: u64) {
        match self {
            Num::Idle => p.idle_secs = Some(v),
            Num::Age => p.age_secs = Some(v),
            Num::Keep => p.keep = Some(v.min(u64::from(u32::MAX)) as u32),
            Num::Superseded => p.superseded_secs = Some(v),
        }
    }

    /// Its value as people say it.
    pub fn shown(self, v: u64) -> String {
        match self {
            Num::Keep => v.to_string(),
            _ => span(v),
        }
    }
}

/// The numbers a kind of rule goes by; none for a tool's command, which
/// either runs at a level or doesn't.
pub fn nums(k: &Kind) -> &'static [Num] {
    match k {
        Kind::CargoArtifacts => &[Num::Age, Num::Keep, Num::Superseded],
        Kind::ToolCommand { .. } => &[],
        _ => &[Num::Idle],
    }
}

fn params(r: &Rule, lv: Level) -> Option<&Params> {
    match lv {
        Level::Normal => r.normal.as_ref(),
        Level::Low => r.low.as_ref(),
    }
}

fn params_mut(r: &mut Rule, lv: Level) -> &mut Option<Params> {
    match lv {
        Level::Normal => &mut r.normal,
        Level::Low => &mut r.low,
    }
}

/// What a rule does at a level, in a few characters, for its row: "2d";
/// artifacts' age, the builds kept and how long superseded, "6h, 3 past
/// 1h"; "runs"; or "–" where it doesn't run.
pub fn level_short(r: &Rule, lv: Level) -> String {
    let Some(p) = params(r, lv) else {
        return "–".into();
    };
    match r.kind {
        Kind::ToolCommand { .. } => "runs".into(),
        Kind::CargoArtifacts => {
            let s = |n: Num| Num::get(n, p).map_or("?".into(), span_short);
            format!(
                "{}, {} past {}",
                s(Num::Age),
                Num::Keep.get(p).map_or("?".into(), |k| k.to_string()),
                s(Num::Superseded)
            )
        }
        _ => Num::Idle.get(p).map_or("?".into(), span_short),
    }
}

/// What a rule does at a level, in words.
pub fn level_text(r: &Rule, lv: Level) -> String {
    let Some(p) = params(r, lv) else {
        return "doesn't run".into();
    };
    match r.kind {
        Kind::ToolCommand { .. } => "runs once a run".into(),
        Kind::CargoArtifacts => {
            let s = |n: Num| Num::get(n, p).map_or("?".into(), span);
            format!(
                "artifacts older than {}; of each crate's builds the newest {} stay, older ones go once superseded {}",
                s(Num::Age),
                Num::Keep.get(p).map_or("?".into(), |k| k.to_string()),
                s(Num::Superseded)
            )
        }
        _ => match Num::Idle.get(p) {
            Some(0) => "whenever it runs".into(),
            Some(v) => format!("after {} idle", span(v)),
            None => "?".into(),
        },
    }
}

/// A command as a shell would take it, each word quoted where it must be.
pub fn command_line(c: &[String]) -> String {
    c.iter()
        .map(|a| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,%~".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a rule is about, in a sentence or two; a command in full.
pub fn what_it_does(r: &Rule) -> String {
    match &r.kind {
        Kind::CargoTarget => "A cargo target goes whole once no build touched it for its idle time, unless a session is working on it.".into(),
        Kind::CargoArtifacts => "In a cargo target that stays, deps artifacts and incremental sessions past its age go; of each crate's builds the newest keep stay, and older ones go once superseded that long.".into(),
        Kind::Worktree => "A linked worktree goes once its commits are in its base branch, nothing is uncommitted, nothing changed for its idle time and nothing works there. Its branch stays.".into(),
        Kind::CacheDir { path, why, each, go_clean } => {
            if *go_clean {
                let at = if path.is_empty() { "where Go keeps it".to_string() } else { path.clone() };
                format!("{why}: go clean -modcache, {at}, once nothing in it changed for its idle time.")
            } else if *each {
                format!("{why}: each hex-named folder in {path}, once nothing in it changed for its idle time.")
            } else {
                format!("{why}: {path}, once nothing in it changed for its idle time.")
            }
        }
        Kind::ToolCommand { command, why } => {
            let mut s = format!("{why}. Runs: {}", command_line(command));
            if !known_tool(command) {
                s.push_str(&format!(
                    " It never runs: its program isn't one rush runs ({}).",
                    KNOWN_TOOLS.join(", ")
                ));
            }
            s
        }
        Kind::PluginCandidate { kind } => {
            format!("What plugins report as {kind} goes once idle.")
        }
    }
}

// ---- Built-in rules, and changes to the file ----

/// The built-in rule of id; None for one of yours.
pub fn built_in(id: &str) -> Option<Rule> {
    built_in_rules().into_iter().find(|r| r.id == id)
}

/// A built-in rule as rush has it, when r is one you changed.
pub fn changed_built_in(r: &Rule) -> Option<Rule> {
    built_in(&r.id).filter(|b| b != r)
}

/// Where a change goes: a rule, or a proposal waiting, by its rule's id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Rule(String),
    Proposal(String),
}

impl Target {
    /// Its rule in f, to change.
    pub fn rule_mut<'a>(&self, f: &'a mut RulesFile) -> Option<&'a mut Rule> {
        match self {
            Target::Rule(id) => f.rules.iter_mut().find(|r| r.id == *id),
            Target::Proposal(id) => f
                .proposed
                .iter_mut()
                .map(|p| &mut p.rule)
                .find(|r| r.id == *id),
        }
    }

    /// Its rule in f.
    pub fn rule<'a>(&self, f: &'a RulesFile) -> Option<&'a Rule> {
        match self {
            Target::Rule(id) => f.rules.iter().find(|r| r.id == *id),
            Target::Proposal(id) => f.proposed.iter().map(|p| &p.rule).find(|r| r.id == *id),
        }
    }
}

/// Moves the proposal for rule id into rules, on; a rule of that id
/// already there is replaced. False when there's no such proposal.
pub fn accept(f: &mut RulesFile, id: &str) -> bool {
    let Some(i) = f.proposed.iter().position(|p| p.rule.id == id) else {
        return false;
    };
    let mut r = f.proposed.remove(i).rule;
    r.enabled = true;
    match f.rules.iter_mut().find(|o| o.id == r.id) {
        Some(o) => *o = r,
        None => f.rules.push(r),
    }
    true
}

/// Drops the proposal for rule id. False when there's none.
pub fn dismiss(f: &mut RulesFile, id: &str) -> bool {
    let n = f.proposed.len();
    f.proposed.retain(|p| p.rule.id != id);
    f.proposed.len() != n
}

/// Puts the built-in rule id back as rush has it. False when it isn't one.
pub fn reset(f: &mut RulesFile, id: &str) -> bool {
    let Some(b) = built_in(id) else {
        return false;
    };
    match f.rules.iter_mut().find(|r| r.id == id) {
        Some(r) => *r = b,
        None => f.rules.push(b),
    }
    true
}

/// Removes a rule of yours; a built-in one can only be turned off.
pub fn remove(f: &mut RulesFile, id: &str) -> bool {
    if built_in(id).is_some() {
        return false;
    }
    let n = f.rules.len();
    f.rules.retain(|r| r.id != id);
    f.rules.len() != n
}

/// Turns a rule on or off.
pub fn switch(f: &mut RulesFile, id: &str) -> Option<bool> {
    let r = f.rules.iter_mut().find(|r| r.id == id)?;
    r.enabled = !r.enabled;
    Some(r.enabled)
}

/// Sets one of t's numbers at lv; a level it didn't run at starts from
/// the other level's numbers.
pub fn set_num(f: &mut RulesFile, t: &Target, lv: Level, n: Num, v: u64) -> bool {
    let Some(r) = t.rule_mut(f) else {
        return false;
    };
    let other = params(
        r,
        if lv == Level::Normal {
            Level::Low
        } else {
            Level::Normal
        },
    )
    .cloned();
    let p = params_mut(r, lv).get_or_insert_with(|| other.unwrap_or_default());
    n.put(p, v);
    true
}

/// Has t run at lv or not; turned on, it starts from the other level's
/// numbers, else the built-in rule's at lv, else none set.
pub fn run_at(f: &mut RulesFile, t: &Target, lv: Level, on: bool) -> bool {
    let Some(r) = t.rule_mut(f) else {
        return false;
    };
    if !on {
        *params_mut(r, lv) = None;
        return true;
    }
    if params(r, lv).is_some() {
        return true;
    }
    let other = if lv == Level::Normal {
        r.low.clone()
    } else {
        r.normal.clone()
    };
    let from = other
        .or_else(|| built_in(&r.id).and_then(|b| params(&b, lv).cloned()))
        .unwrap_or_default();
    *params_mut(r, lv) = Some(from);
    true
}

// ---- The runs, from the event log ----

/// A disk-clean event's did or left line, as event_fields writes it:
/// "path: what (rule, freed)" or "path: why (rule)". The rule, the text
/// (the path left off) and the bytes freed, if it says.
fn parse_entry(s: &str, did: bool) -> Option<(String, String, u64)> {
    let (head, tail) = s.rsplit_once(" (")?;
    let inner = tail.strip_suffix(')')?;
    let (rule, freed) = if did {
        let (r, f) = inner.rsplit_once(", ")?;
        (r.to_string(), f.parse().unwrap_or(0))
    } else {
        (inner.to_string(), 0)
    };
    let text = match head.split_once(": ") {
        Some((p, t)) if p.starts_with('/') || p.starts_with('~') => t,
        _ => head,
    };
    Some((rule, text.to_string(), freed))
}

/// What the runs did, from their events, newest first: (when, fields).
pub fn tally(events: &[(Ms, Value)]) -> Runs {
    let mut out = Runs {
        n: events.len(),
        ..Runs::default()
    };
    let u = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let s = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let list = |v: &Value, k: &str| -> Vec<String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (i, (at, v)) in events.iter().enumerate() {
        if i == 0 {
            out.last = Some(LastRun {
                at: *at,
                free: u(v, "free"),
                level: s(v, "level"),
                freed: u(v, "freed"),
                why: s(v, "why"),
            });
        }
        for d in list(v, "did") {
            if let Some((rule, _, freed)) = parse_entry(&d, true) {
                let t = out.by_rule.entry(rule).or_default();
                t.freed += freed;
                t.did += 1;
            }
        }
        // Only the newest run that left something of a rule says what it
        // leaves now.
        let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
        for l in list(v, "left") {
            if let Some((rule, why, _)) = parse_entry(&l, false) {
                let e = seen.entry(rule).or_insert((0, why));
                e.0 += 1;
            }
        }
        for (rule, (n, why)) in seen {
            let t = out.by_rule.entry(rule).or_default();
            if t.left == 0 && t.why.is_empty() {
                t.left = n;
                t.why = why;
            }
        }
    }
    out
}

/// The newest disk-clean events in rush's log, summed up.
fn read_runs() -> Runs {
    let f = rush_events::Filter {
        kinds: vec![EVENT.into()],
        limit: RUNS,
        ..rush_events::Filter::default()
    };
    let Ok((lines, _)) = rush_events::query(&rush_events::path(), &f) else {
        return Runs::default();
    };
    let evs: Vec<(Ms, Value)> = lines
        .iter()
        .filter_map(|l| Some((l.at, serde_json::from_slice(&l.raw).ok()?)))
        .collect();
    tally(&evs)
}

// ---- Reading and writing, off the UI ----

/// Reads the rules and the runs again, off the UI, when the last reading
/// is more than a moment old.
pub fn want(m: &mut Model) {
    let st = &mut m.kept::<State>().disk;
    if st.reading || st.at.is_some_and(|a| a.elapsed() < FRESH) {
        return;
    }
    st.reading = true;
    m.later(
        || (load_rules().map_err(|e| e.to_string()), read_runs()),
        |m, (file, runs)| {
            let st = &mut m.kept::<State>().disk;
            st.at = Some(Instant::now());
            st.reading = false;
            st.file = Some(file);
            st.runs = runs;
            Out::Draw
        },
    );
}

/// Changes the rules file off the UI, under its lock, and says said once
/// it's written; f says false when there was nothing to change.
fn change(
    m: &mut Model,
    said: String,
    f: impl FnOnce(&mut RulesFile) -> bool + Send + 'static,
) -> Out {
    m.later(
        move || {
            let mut did = false;
            let r = update_rules(|file| did = f(file)).map_err(|e| e.to_string());
            (r, did)
        },
        move |m, (r, did)| {
            match r {
                Err(e) => m.fail(&format!("couldn't change the disk rules: {e}")),
                Ok(file) => {
                    let st = &mut m.kept::<State>().disk;
                    st.file = Some(Ok(file));
                    st.at = Some(Instant::now());
                    if did {
                        m.flash(&said);
                    }
                }
            }
            Out::Draw
        },
    );
    Out::Draw
}

fn snap(m: &Model) -> Snap {
    m.kept
        .peek::<State>()
        .map(|s| s.disk.clone())
        .unwrap_or_default()
}

// ---- The page ----

/// GB in a few characters, a level's edge.
fn gb(n: u64) -> String {
    format!("{}G", n >> 30)
}

/// The page's sections.
pub fn sections(m: &Model) -> Vec<Section> {
    let snap = snap(m);
    let mut top = vec![
        auto_row(m),
        level_row(
            "Normal",
            NORMAL_UNDER,
            "rules run with their normal numbers",
        ),
        level_row(
            "Low",
            LOW_UNDER,
            "rules run with their low numbers; some run only then",
        ),
        last_run_row(m, &snap.runs),
    ];
    if RESEARCH.get().is_some() {
        top.push(research_row());
    }
    let mut out = vec![Section {
        title: "Disk".into(),
        note: "rush cleans by the rules below, removing only what they say goes".into(),
        rows: top,
        ..Section::default()
    }];
    out.push(when_short(m));
    out.push(quiet(m));
    let file = match &snap.file {
        None => {
            out.push(Section::new(
                "Rules",
                vec![note_row("reading the rules…", DIM)],
            ));
            return out;
        }
        Some(Err(e)) => {
            out.push(Section::new(
                "Rules",
                vec![note_row(
                    &format!("the rules can't be read, so none run: {e}"),
                    RED,
                )],
            ));
            return out;
        }
        Some(Ok(f)) => f,
    };
    let cols = Cols::of(
        file.rules
            .iter()
            .chain(file.proposed.iter().map(|p| &p.rule)),
    );
    out.push(Section {
        title: "Rules".into(),
        note: if snap.runs.n == 0 {
            "normal · low · no runs yet".into()
        } else {
            format!("normal · low · what the last {} runs did", snap.runs.n)
        },
        rows: file
            .rules
            .iter()
            .map(|r| rule_row(r, &snap.runs, &snap.removing, cols))
            .collect(),
        ..Section::default()
    });
    let proposed: Vec<Setting> = file
        .proposed
        .iter()
        .map(|p| proposal_row(m, p, &snap.removing, cols))
        .collect();
    out.push(Section {
        title: "Proposed".into(),
        note: "waiting for your yes; none runs until accepted".into(),
        rows: if proposed.is_empty() {
            vec![note_row(
                "none waiting · the cleanup agent's research proposes rules here",
                DIM,
            )]
        } else {
            proposed
        },
        ..Section::default()
    });
    out
}

/// What rush does when space runs short, whatever the switch says: offer
/// a cleanup (care.rs), the nearly-full clean (dire.rs: the rules below
/// at low, then its own set), and trimming a running session's temp
/// files (rush-host's trim.rs). Their keys are config.json's as they were
/// on General: cleanupDiskGB, cleanupDireGB, trimRunningTmp.
fn when_short(m: &Model) -> Section {
    let c = config(m);
    let mut offer = choice(
        "Offer a cleanup when disk is under",
        &off_pick(c.cleanup_disk_gb, "G"),
        "rush watches your free disk itself, without AI, whether Clean on its own is on or off. When there's less than this it asks, at most every 6 hours, what you'd like: Clean up now opens rush's own list with no agent (worktrees that can go, done sessions' scratch, and caches that rebuild themselves: ones rush knows, and ones you accepted under Proposed), ticked for you to confirm; or a cleanup agent (with the cleanup plugin on; off, it recommends turning it on) reads your worktrees, rush's scratch folders, what's left in /tmp and what plugins like Haven and go-clean report, and proposes a list of what can go without losing work for you to approve. Nothing is removed until you say yes, and then only what you approve.",
        &[
            ("", "it asks once there's less than 20G free."),
            ("10G", "it asks once there's less than 10G free."),
            ("50G", "it asks once there's less than 50G free."),
            ("off", "it never asks about disk."),
        ],
        set(|m, v| config_mut(m).cleanup_disk_gb = off_value(v)),
    );
    offer.unset = "20G".into();
    let mut dire = choice(
        "When nearly full, clean anyway under",
        &off_pick(c.cleanup_dire_gb, "G"),
        &format!(
            "A last resort, apart from Clean on its own above: it runs even with that off. When free disk is under this, you haven't touched rush for 30 minutes and a cleanup's list (if there is one) has waited as long, rush cleans in two steps. First the rules below, at their low numbers (under {} free), all but the worktree rule; a rule you turned off stays off. Then, only if there still isn't room, its own set of what's made again and loses no work: done sessions' scratch folders untouched for days, build caches (target, node_modules, .next, .turbo, dist) in worktrees nobody works in, then pnpm's unused store and docker's prunes. Biggest first, only until there's room again. Never a worktree, a stopped session's scratch, or anything changed lately; the rest of the list still waits for you.",
            gb(LOW_UNDER)
        ),
        &[
            ("", "under 5G free, or 2% of the disk if that's less."),
            ("2G", "under 2G free."),
            ("10G", "under 10G free."),
            (
                "off",
                "rush never cleans on its own because the disk is nearly full.",
            ),
        ],
        set(|m, v| config_mut(m).cleanup_dire_gb = off_value(v)),
    );
    dire.unset = "5G".into();
    let trim = choice(
        "Trim a running session's temp files",
        on_off(c.trim_running_tmp),
        "Each session has a temp folder of its own. With this on, while free disk is under 20G (or under a tenth of the disk), whether Clean on its own is on or off, rush looks every 10 minutes and removes what's in a running session's temp folder that's 3 hours old and open in no process. Claude Code's own folders there stay. A session that's ended has its temp folder removed anyway.",
        &[
            ("off", "a running session's temp files stay until it ends."),
            (
                "on",
                "under 20G free, its old temp files that nothing has open go.",
            ),
        ],
        set(|m, v| config_mut(m).trim_running_tmp = v == "on"),
    );
    Section {
        title: "When space runs short".into(),
        note: "whether Clean on its own is on or off".into(),
        rows: vec![offer, dire, trim],
        ..Section::default()
    }
}

/// Folders kept out of Spotlight and Time Machine (rush-fleet's
/// disk::quiet): quietScratch, on unless turned off, and quietProjects,
/// off unless turned on.
fn quiet(m: &Model) -> Section {
    let c = config(m);
    let scratch = choice(
        "Keep scratch out of Spotlight and Time Machine",
        on_off(c.quiets_scratch()),
        "Builds and agents write thousands of files a minute in rush's scratch: its tmp folders, each repo's .claude/worktrees and Rust target folders. macOS tells Spotlight and Time Machine of every write, and they go and look. With this on, rush (the one that leads, once an hour) puts a .metadata_never_index file in each, which Spotlight skips, and excludes each from Time Machine backups (tmutil addexclusion, no password). git is told to ignore the file, so it never shows as untracked. Nothing is moved or removed. macOS can't stop telling of the writes; this stops what reads them doing the work. Turning it off doesn't undo what was done: tmutil removeexclusion and removing the file do.",
        &[
            (
                "on",
                "scratch isn't indexed or backed up: worktrees with work not committed aren't backed up either.",
            ),
            ("off", "rush leaves Spotlight and Time Machine alone."),
        ],
        set(|m, v| config_mut(m).quiet_scratch = Some(v == "on")),
    );
    let projects = choice(
        "Keep your projects out of them too",
        on_off(c.quiet_projects),
        "The same for the checkouts of every repo rush's sessions work in, main checkout and worktrees alike: Spotlight won't find your code, and Time Machine won't back it up. For code that lives in git and is pushed anyway. Turning it off doesn't undo what was done: tmutil removeexclusion on the folder and removing its .metadata_never_index do.",
        &[
            ("off", "your projects stay searchable and backed up."),
            (
                "on",
                "your projects aren't indexed or backed up: only what's pushed is kept elsewhere.",
            ),
        ],
        set(|m, v| config_mut(m).quiet_projects = v == "on"),
    );
    Section {
        title: "Spotlight and Time Machine".into(),
        note: "macOS only".into(),
        rows: vec![scratch, projects],
        ..Section::default()
    }
}

/// A line of the page with nothing to change.
fn note_row(text: &str, c: Color) -> Setting {
    let t = text.to_string();
    Setting {
        label: t.clone(),
        line: Some(Rc::new(move |w| trunc(line([sp(t.clone(), c)]), w))),
        ..Setting::default()
    }
}

/// The switch.
fn auto_row(m: &Model) -> Setting {
    let on = config(m).disk.auto;
    let mut st = choice(
        "Clean on its own when disk is low",
        if on { "on" } else { "off" },
        &format!(
            "Rush runs these rules itself while free disk is under {} (harder under {}), and logs each run as a disk-clean event saying what went and what stayed and why. Off, rush runs them by itself only as a last resort, when the disk is nearly full (below, When nearly full: a switch of its own); Clean up still offers the same rules.",
            gb(NORMAL_UNDER),
            gb(LOW_UNDER)
        ),
        &[
            (
                "off",
                "the rules run on their own only in the nearly-full clean below.",
            ),
            (
                "on",
                "rush cleans by the rules when the disk is low, removing nothing in use.",
            ),
        ],
        set(|m, v| config_mut(m).disk.auto = v == "on"),
    );
    st.unset = "off".into();
    st
}

/// One of the levels, as it is (fixed for now).
fn level_row(name: &str, under: u64, what: &str) -> Setting {
    let styled = line([
        sp(format!("under {} free", gb(under)), TEXT),
        sp(format!(" · {what}"), DIM),
    ]);
    Setting {
        label: name.into(),
        value: styled.plain(),
        styled,
        what: format!(
            "The {} level: free disk under {}. Each rule has numbers for each level; a level a rule has none for is one it doesn't run at.",
            name.to_lowercase(),
            gb(under)
        ),
        ..Setting::default()
    }
}

/// The newest run.
fn last_run_row(m: &Model, runs: &Runs) -> Setting {
    let styled = match &runs.last {
        None => line([sp("none yet", DIM)]),
        Some(l) => {
            let mut s = line([sp(
                format!("{} ago", age_since((m.now - l.at).max(0))),
                TEXT,
            )]);
            if l.free > 0 {
                s.spans
                    .push(sp(format!(" · {} free", disk(l.free as i64)), DIM));
            }
            if !l.level.is_empty() {
                s.spans.push(sp(format!(" · {}", l.level), DIM));
            }
            if l.freed > 0 {
                s.spans
                    .push(sp(format!(" · freed {}", disk(l.freed as i64)), GREEN));
            } else if !l.why.is_empty() {
                s.spans.push(sp(format!(" · {}", l.why), DIM));
            } else {
                s.spans.push(sp(" · freed nothing", DIM));
            }
            s
        }
    };
    Setting {
        label: "Last run".into(),
        value: styled.plain(),
        styled,
        what: "The engine's newest run: when, the disk's free space and level then, and what it freed. rush events -k disk-clean shows each run whole.".into(),
        ..Setting::default()
    }
}

/// Research now: the cleanup agent looks for more to propose.
fn research_row() -> Setting {
    Setting {
        label: "Research now".into(),
        value: "the cleanup agent looks for more rules to propose".into(),
        what: "Starts the cleanup agent in research mode: it looks at what fills the disk and who made it, and proposes rules under Proposed with its evidence. It removes nothing; a rule runs only once you accept it.".into(),
        keys: vec!["enter".into(), "research".into()],
        key: Some(Rc::new(|m: &mut Model, s: &str| {
            if s != "enter" {
                return Out::Pass;
            }
            match RESEARCH.get() {
                Some(f) => f(m),
                None => Out::Pass,
            }
        })),
        ..Setting::default()
    }
}

/// The rule rows' column widths: titles, then each level's numbers.
#[derive(Clone, Copy)]
struct Cols {
    title: usize,
    normal: usize,
    low: usize,
}

impl Cols {
    fn of<'a>(rules: impl Iterator<Item = &'a Rule>) -> Cols {
        let mut c = Cols {
            title: 12,
            normal: 6,
            low: 3,
        };
        for r in rules {
            c.title = c.title.max(width(&r.title));
            c.normal = c.normal.max(width(&level_short(r, Level::Normal)));
            c.low = c.low.max(width(&level_short(r, Level::Low)));
        }
        c.title = c.title.min(32);
        c
    }
}

/// A rule's line: on or off, its title, its numbers at each level, and
/// what it did in the runs read.
pub fn rule_line(r: &Rule, t: Option<&Tally>, w: usize) -> Line {
    rule_line_in(r, t, Cols::of(std::iter::once(r)), w)
}

fn rule_line_in(r: &Rule, t: Option<&Tally>, c: Cols, w: usize) -> Line {
    let (mark, tc) = if r.enabled {
        (sp("● ", GREEN), TEXT)
    } else {
        (sp("○ ", FAINT), DIM)
    };
    let mut l = line([
        mark,
        sp(fit_str(&r.title, c.title), tc),
        plain("  "),
        sp(fit_str(&level_short(r, Level::Normal), c.normal), SUB),
        plain("  "),
        sp(fit_str(&level_short(r, Level::Low), c.low), SUB),
    ]);
    if changed_built_in(r).is_some() {
        l.spans.push(sp("  changed", YELLOW));
    } else if built_in(&r.id).is_none() {
        l.spans.push(sp("  yours", BLUE));
    }
    if let Some(t) = t {
        if t.freed > 0 {
            l.spans
                .push(sp(format!("  freed {}", disk(t.freed as i64)), GREEN));
        } else if t.did > 0 {
            l.spans.push(sp(format!("  ran {}×", t.did), DIM));
        }
        if t.left > 0 {
            l.spans.push(sp(format!("  left {}", t.left), DIM));
            if !t.why.is_empty() {
                l.spans.push(sp(format!(": {}", t.why), FAINT));
            }
        }
    }
    trunc(l, w)
}

/// The About text of a rule: what it does, then its numbers and runs.
fn rule_about(r: &Rule, t: Option<&Tally>, waiting: bool) -> (String, String, String) {
    let mut now = format!(
        "Normal: {}. Low: {}.",
        level_text(r, Level::Normal),
        level_text(r, Level::Low)
    );
    if !r.enabled {
        now.push_str(" Off.");
    }
    if let Some(b) = changed_built_in(r) {
        now.push_str(&format!(
            " Changed from rush's: normal {}, low {}{}.",
            level_short(&b, Level::Normal),
            level_short(&b, Level::Low),
            if b.enabled == r.enabled { "" } else { ", on" }
        ));
    }
    if !waiting {
        match t {
            Some(t) if t.did > 0 || t.left > 0 => {
                if t.did > 0 {
                    now.push_str(&format!(
                        " Lately: {} done, {} freed.",
                        t.did,
                        disk(t.freed as i64)
                    ));
                }
                if t.left > 0 {
                    now.push_str(&format!(" Left {}, first: {}.", t.left, t.why));
                }
            }
            _ => now.push_str(" Nothing done lately."),
        }
    }
    (r.title.clone(), what_it_does(r), now)
}

/// A rule's row: space turns it on or off, enter its numbers, r puts a
/// changed built-in back, x twice removes one of yours.
fn rule_row(r: &Rule, runs: &Runs, removing: &str, c: Cols) -> Setting {
    let t = runs.by_rule.get(&r.id).cloned();
    let changed = changed_built_in(r).is_some();
    let yours = built_in(&r.id).is_none();
    let mut keys = vec!["space", "on/off"];
    if !nums(&r.kind).is_empty() || matches!(r.kind, Kind::ToolCommand { .. }) {
        keys.extend(["enter", "its numbers"]);
    }
    if changed {
        keys.extend(["r", "reset to default"]);
    }
    if yours {
        keys.extend(["x", "remove"]);
    }
    let asking = removing == r.id;
    let (r1, r2, t1) = (r.clone(), r.clone(), t.clone());
    Setting {
        label: r.title.clone(),
        line: Some(Rc::new(move |w| {
            if asking {
                return line([sp(format!("x again to remove {}", r1.title), RED)]);
            }
            rule_line_in(&r1, t1.as_ref(), c, w)
        })),
        about: Some(Rc::new(move || rule_about(&r2, t.as_ref(), false))),
        keys: keys.into_iter().map(String::from).collect(),
        key: Some({
            let r = r.clone();
            Rc::new(move |m: &mut Model, s: &str| rule_key(m, &r, s))
        }),
        ..Setting::default()
    }
}

fn rule_key(m: &mut Model, r: &Rule, s: &str) -> Out {
    let id = r.id.clone();
    if s != "x" {
        m.kept::<State>().disk.removing.clear();
    }
    match s {
        "space" => {
            let said = format!("{} {}", r.title, if r.enabled { "off" } else { "on" });
            change(m, said, move |f| switch(f, &id).is_some())
        }
        "enter" => {
            numbers(m, Target::Rule(id), r);
            Out::Draw
        }
        "r" if changed_built_in(r).is_some() => {
            let said = format!("{} back to rush's numbers", r.title);
            change(m, said, move |f| reset(f, &id))
        }
        "x" if built_in(&r.id).is_none() => {
            let st = &mut m.kept::<State>().disk;
            if st.removing != id {
                st.removing = id;
                return Out::Draw;
            }
            st.removing.clear();
            change(m, format!("removed {}", r.title), move |f| remove(f, &id))
        }
        _ => Out::Pass,
    }
}

/// A proposal's row: enter looks and accepts, e edits its numbers first,
/// x twice dismisses it.
fn proposal_row(m: &Model, p: &rush_fleet::disk::Proposal, removing: &str, c: Cols) -> Setting {
    let r = p.rule.clone();
    let asking = removing == r.id;
    let mut who = String::new();
    if !p.by.is_empty() {
        who = format!("  by {}", p.by);
    }
    if p.at > 0 {
        who.push_str(&format!(" {} ago", age_since((m.now - p.at).max(0))));
    }
    let (r1, r2, r3) = (r.clone(), r.clone(), r.clone());
    let p2 = p.clone();
    Setting {
        label: r.title.clone(),
        line: Some(Rc::new(move |w| {
            if asking {
                return line([sp(format!("x again to dismiss {}", r1.title), RED)]);
            }
            let mut l = rule_line_in(&r1, None, c, w);
            l.spans.push(sp(who.clone(), DIM));
            trunc(l, w)
        })),
        about: Some(Rc::new(move || {
            let (title, what, now) = rule_about(&r2, None, true);
            let mut what = what;
            if !p2.evidence.is_empty() {
                what.push_str(&format!(" Evidence: {}", p2.evidence));
            }
            (title, what, now)
        })),
        keys: ["enter", "accept", "e", "edit numbers", "x", "dismiss"]
            .map(String::from)
            .to_vec(),
        key: Some({
            let p = p.clone();
            Rc::new(move |m: &mut Model, s: &str| proposal_key(m, &p, &r3, s))
        }),
        ..Setting::default()
    }
}

fn proposal_key(m: &mut Model, p: &rush_fleet::disk::Proposal, r: &Rule, s: &str) -> Out {
    let id = r.id.clone();
    if s != "x" {
        m.kept::<State>().disk.removing.clear();
    }
    match s {
        "enter" | "a" => {
            m.ask(accept_confirm(p));
            Out::Draw
        }
        "e" => {
            numbers(m, Target::Proposal(id), r);
            Out::Draw
        }
        "x" => {
            let st = &mut m.kept::<State>().disk;
            if st.removing != id {
                st.removing = id;
                return Out::Draw;
            }
            st.removing.clear();
            change(m, format!("dismissed {}", r.title), move |f| {
                dismiss(f, &id)
            })
        }
        _ => Out::Pass,
    }
}

/// The question accepting asks: the rule whole, a command word for word,
/// its numbers, and what shows it's safe.
pub fn accept_confirm(p: &rush_fleet::disk::Proposal) -> Confirm {
    let r = &p.rule;
    let mut detail = format!(
        "{} Normal: {}. Low: {}.",
        what_it_does(r),
        level_text(r, Level::Normal),
        level_text(r, Level::Low)
    );
    if !p.evidence.is_empty() {
        detail.push_str(&format!(" Evidence: {}", p.evidence));
    }
    if !p.by.is_empty() {
        detail.push_str(&format!(" Proposed by {}.", p.by));
    }
    let (id, title) = (r.id.clone(), r.title.clone());
    let question = format!("Accept “{title}” as a rule?");
    let mut c = Confirm::new(question, move |m: &mut Model| {
        change(m, format!("{title} is a rule now, on"), move |f| {
            accept(f, &id)
        })
    })
    .detail(detail);
    c.yes_text = "accept".into();
    c
}

/// Opens t's numbers in a picker: each number at each level (enter types
/// a new one), and whether it runs at each.
fn numbers(m: &mut Model, t: Target, r: &Rule) {
    let mut rows = Vec::new();
    for lv in [Level::Normal, Level::Low] {
        let name = lv.name();
        match params(r, lv) {
            None => rows.push(Pick {
                label: line([
                    sp(format!("{name:<7}"), SUB),
                    sp("doesn't run", DIM),
                    sp(" · enter has it run", FAINT),
                ]),
                find: format!("{name} run"),
                value: format!("{name}:on"),
                section: String::new(),
            }),
            Some(p) => {
                for n in nums(&r.kind) {
                    let v = n.get(p).map_or("not set".into(), |v| n.shown(v));
                    rows.push(Pick {
                        label: line([
                            sp(format!("{name:<7}"), SUB),
                            sp(format!("{:<11}", n.name()), DIM),
                            sp(v, TEXT),
                        ]),
                        find: format!("{name} {}", n.name()),
                        value: format!("{name}:{}", n.name()),
                        section: String::new(),
                    });
                }
                rows.push(Pick {
                    label: line([
                        sp(format!("{name:<7}"), SUB),
                        sp(
                            if nums(&r.kind).is_empty() {
                                "runs · enter stops it at this level"
                            } else {
                                "stop it running at this level"
                            },
                            DIM,
                        ),
                    ]),
                    find: format!("{name} stop"),
                    value: format!("{name}:off"),
                    section: String::new(),
                });
            }
        }
    }
    let (title, r2) = (r.title.clone(), r.clone());
    let mut p = Picker::new(
        format!("{title}'s numbers"),
        rows,
        Box::new(move |m, pick, _| {
            let Some(pick) = pick else {
                return Out::Idle;
            };
            picked(m, t.clone(), &r2, &pick.value)
        }),
    );
    p.about = format!(
        "Normal is under {} free, low under {}. Type a while as 30m, 6h, 2d or 1w.",
        gb(NORMAL_UNDER),
        gb(LOW_UNDER)
    );
    p.keys = vec![
        ("↑↓".into(), "choose".into()),
        ("enter".into(), "change it".into()),
        ("esc".into(), "keep them".into()),
    ];
    m.open_sheet(Box::new(p));
}

fn picked(m: &mut Model, t: Target, r: &Rule, value: &str) -> Out {
    let Some((lv, what)) = value.split_once(':') else {
        return Out::Idle;
    };
    let lv = if lv == "low" {
        Level::Low
    } else {
        Level::Normal
    };
    let title = r.title.clone();
    match what {
        "on" | "off" => {
            let on = what == "on";
            let said = format!(
                "{title} {} at {}",
                if on { "runs" } else { "doesn't run" },
                lv.name()
            );
            change(m, said, move |f| run_at(f, &t, lv, on))
        }
        n => {
            let Some(n) = Num::of(n) else {
                return Out::Idle;
            };
            let cur = params(r, lv).and_then(|p| n.get(p));
            let prefill = match (n, cur) {
                (_, None) => String::new(),
                (Num::Keep, Some(v)) => v.to_string(),
                (_, Some(v)) => span_short(v),
            };
            let what = match n {
                Num::Keep => format!("{} {} (builds)", lv.name(), n.name()),
                _ => format!("{} {} (30m, 6h, 2d)", lv.name(), n.name()),
            };
            ask(
                m,
                &what,
                &prefill,
                Rc::new(move |m: &mut Model, input: &str| {
                    let v = match n {
                        Num::Keep => input
                            .trim()
                            .parse::<u64>()
                            .map_err(|_| format!("{input:?} isn't a number of builds")),
                        _ => parse_span(input),
                    };
                    let v = match v {
                        Ok(v) => v,
                        Err(e) => {
                            m.flash_err(&e);
                            return Out::Draw;
                        }
                    };
                    let said = format!("{title}: {} {} {}", lv.name(), n.name(), n.shown(v));
                    let t = t.clone();
                    change(m, said, move |f| set_num(f, &t, lv, n, v))
                }),
            );
            Out::Draw
        }
    }
}
