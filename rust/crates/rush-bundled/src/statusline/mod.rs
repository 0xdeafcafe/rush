//! rush's status line for Claude Code: the layout you build in rush's
//! /statusline, and `rush statusline`, the command Claude Code runs to
//! draw it (session JSON on stdin, one line out).
//! Ported from internal/statusline/statusline.go:1-585.

mod bars;

pub use bars::*;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use rush_core::usage::Quota;
use rush_plugin::Ctx;
use rush_state::{LOGINS_KIND, Time};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Which segments show on which line, in order, and how they're joined.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    #[serde(serialize_with = "nil_if_empty", deserialize_with = "lines")]
    pub lines: Vec<Vec<String>>,
    pub sep: String,
    /// no colour
    #[serde(skip_serializing_if = "is_false")]
    pub plain: bool,
    /// A status line command of your own, drawn by the "custom" segment:
    /// the one rush's replaced, kept.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Lines none, as Go's nil, are written null.
fn nil_if_empty<S: Serializer>(v: &[Vec<String>], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_none();
    }
    v.serialize(s)
}

/// Lines as Go reads them: null, and a null line, read as none.
fn lines<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<String>>, D::Error> {
    let v: Option<Vec<Option<Vec<String>>>> = Option::deserialize(d)?;
    Ok(v.unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

/// How many lines the builder offers.
pub const MAX_LINES: usize = 3;

fn layout(lines: &[&[&str]], sep: &str) -> Layout {
    Layout {
        lines: lines
            .iter()
            .map(|l| l.iter().map(|s| s.to_string()).collect())
            .collect(),
        sep: sep.into(),
        ..Default::default()
    }
}

/// What a new status line starts as.
pub fn default_layout() -> Layout {
    layout(
        &[&["model", "folder", "branch", "context", "cost", "health"]],
        " · ",
    )
}

impl Layout {
    /// Whether a segment is on any line.
    pub fn shown(&self, id: &str) -> bool {
        self.lines.iter().any(|l| l.iter().any(|s| s == id))
    }
}

/// The separators the builder offers.
pub const SEPS: [&str; 5] = [" · ", " │ ", "  ", " / ", " ▸ "];

/// What Claude Code sends the status line command.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub session_id: String,
    pub cwd: String,
    pub model: Model,
    pub effort: Effort,
    pub workspace: Workspace,
    pub version: String,
    pub output_style: OutputStyle,
    pub cost: Cost,
    #[serde(rename = "context_window")]
    pub context: Context,
    #[serde(rename = "fast_mode")]
    pub fast: bool,
    /// as it came, for a command of your own
    #[serde(skip)]
    raw: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Effort {
    pub level: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub current_dir: String,
    pub project_dir: String,
    pub repo: Option<Repo>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputStyle {
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cost {
    #[serde(rename = "total_cost_usd")]
    pub usd: f64,
    #[serde(rename = "total_duration_ms")]
    pub duration_ms: f64,
    #[serde(rename = "total_lines_added")]
    pub lines_added: i64,
    #[serde(rename = "total_lines_removed")]
    pub lines_removed: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Context {
    #[serde(rename = "total_input_tokens")]
    pub input: i64,
    #[serde(rename = "context_window_size")]
    pub size: i64,
    #[serde(rename = "used_percentage")]
    pub used_pct: Option<f64>,
}

/// What the line knows beyond the input: found out before it's drawn, so
/// drawing never waits.
#[derive(Clone, Default)]
struct Extra {
    custom: String,
    branch: String,
    usage: Option<Quota>,
    acct: String,
    /// the providers failing, as the health segment says them
    failing: String,
    now: Time,
    /// your own command's output
    own: String,
}

const ORANGE: &str = "\x1b[38;2;217;119;87m";
const BLUE: &str = "\x1b[38;2;122;162;247m";
const GREEN: &str = "\x1b[38;2;130;190;120m";
const RED: &str = "\x1b[38;2;224;108;117m";
const YELLOW: &str = "\x1b[38;2;229;192;123m";
const GREY: &str = "\x1b[38;2;168;162;152m";
const DIM: &str = "\x1b[38;2;122;117;108m";
const RESET: &str = "\x1b[0m";

fn level(p: f64) -> &'static str {
    if p >= 85.0 {
        RED
    } else if p >= 60.0 {
        YELLOW
    } else {
        GREEN
    }
}

type Draw = fn(&Input, &Extra) -> (String, &'static str);

/// One thing the line can show.
#[derive(Clone, Copy)]
pub struct Segment {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    draw: Draw,
}

fn none() -> (String, &'static str) {
    (String::new(), "")
}

/// Every segment, in the order the builder lists them.
pub static SEGMENTS: &[Segment] = &[
    Segment {
        id: "model",
        name: "Model",
        about: "the model this session runs",
        draw: |i, _| {
            (
                model_name(first_of(&[&i.model.display_name, &i.model.id])),
                ORANGE,
            )
        },
    },
    Segment {
        id: "effort",
        name: "Effort",
        about: "the effort level",
        draw: |i, _| (i.effort.level.clone(), GREY),
    },
    Segment {
        id: "folder",
        name: "Folder",
        about: "the folder it works in",
        draw: |i, _| {
            let d = first_of(&[&i.workspace.current_dir, &i.cwd]);
            if d.is_empty() {
                return none();
            }
            (base(d), BLUE)
        },
    },
    Segment {
        id: "repo",
        name: "Repository",
        about: "owner/name of the git remote",
        draw: |i, _| match &i.workspace.repo {
            Some(r) if !r.name.is_empty() => (format!("{}/{}", r.owner, r.name), BLUE),
            _ => none(),
        },
    },
    Segment {
        id: "branch",
        name: "Git branch",
        about: "the branch checked out",
        draw: |_, x| {
            if x.branch.is_empty() {
                return none();
            }
            (format!("⎇ {}", x.branch), GREEN)
        },
    },
    Segment {
        id: "context",
        name: "Context",
        about: "how full the context window is",
        draw: |i, _| {
            let p = match i.context.used_pct {
                Some(p) => p,
                None if i.context.size > 0 && i.context.input > 0 => {
                    100.0 * i.context.input as f64 / i.context.size as f64
                }
                None => -1.0,
            };
            if p < 0.0 {
                return none();
            }
            (format!("ctx {p:.0}%"), level(p))
        },
    },
    Segment {
        id: "cost",
        name: "Cost",
        about: "what the session has cost so far",
        draw: |i, _| {
            if i.cost.usd <= 0.0 {
                return none();
            }
            (format!("${:.2}", i.cost.usd), YELLOW)
        },
    },
    Segment {
        id: "duration",
        name: "Time",
        about: "how long the session has run",
        draw: |i, _| {
            if i.cost.duration_ms <= 0.0 {
                return none();
            }
            (short(i.cost.duration_ms as i64 / 1000), GREY)
        },
    },
    Segment {
        id: "lines",
        name: "Lines changed",
        about: "lines added and removed",
        draw: |i, _| {
            if i.cost.lines_added + i.cost.lines_removed == 0 {
                return none();
            }
            (
                format!(
                    "{GREEN}+{}{RESET} {RED}−{}",
                    i.cost.lines_added, i.cost.lines_removed
                ),
                "",
            )
        },
    },
    Segment {
        id: "usage",
        name: "Plan usage",
        about: "the 5-hour and weekly limits, as rush last read them",
        draw: |_, x| {
            let Some(u) = &x.usage else {
                return none();
            };
            let parts: Vec<String> = u
                .windows
                .iter()
                .map(|w| format!("{}{} {:.0}%{RESET}", level(w.percent), w.label, w.percent))
                .collect();
            (parts.join(" "), "")
        },
    },
    Segment {
        id: "health",
        name: "Provider health",
        about: "a provider whose login or requests are failing, as “claude ✗ invalid_grant”; nothing while all work",
        draw: |_, x| (x.failing.clone(), RED),
    },
    Segment {
        id: "account",
        name: "Account",
        about: "which Claude account this is",
        draw: |_, x| (x.acct.clone(), GREY),
    },
    Segment {
        id: "style",
        name: "Output style",
        about: "the output style, when it isn't the default",
        draw: |i, _| {
            if i.output_style.name.is_empty() || i.output_style.name == "default" {
                return none();
            }
            (i.output_style.name.clone(), GREY)
        },
    },
    Segment {
        id: "fast",
        name: "Fast mode",
        about: "“fast” while fast mode is on",
        draw: |i, _| {
            if !i.fast {
                return none();
            }
            ("fast".into(), YELLOW)
        },
    },
    Segment {
        id: "version",
        name: "Version",
        about: "Claude Code's version",
        draw: |i, _| {
            if i.version.is_empty() {
                return none();
            }
            (format!("v{}", i.version), DIM)
        },
    },
    Segment {
        id: "clock",
        name: "Clock",
        about: "the time of day",
        draw: |_, x| (clock(x.now), DIM),
    },
    Segment {
        id: "session",
        name: "Session id",
        about: "the conversation's short id",
        draw: |i, _| {
            if i.session_id.len() < 8 || !i.session_id.is_char_boundary(8) {
                return none();
            }
            (i.session_id[..8].to_string(), DIM)
        },
    },
    Segment {
        id: "custom",
        name: "Your own line",
        about: "what your own status line command prints",
        draw: |_, x| {
            if x.custom.is_empty() {
                return none();
            }
            (x.own.clone(), "")
        },
    },
];

/// The time of day, as Go's Format("15:04") writes it, in the time's own
/// zone.
fn clock(t: Time) -> String {
    let s = t.format();
    s.get(11..16).unwrap_or_default().to_string()
}

fn base(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { ".".into() } else { "/".into() };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// How a model is shown: "Opus 5.5" for claude-opus-5-5[1m], as the
/// Claude adapter names it.
pub fn model_name(s: &str) -> String {
    rush_core::adapter::global().model_name(LOGINS_KIND, s)
}

/// Runs your own status line command with the session on stdin, for at
/// most two seconds.
fn run_own(cmd: &str, input: &Input) -> String {
    let raw = match &input.raw {
        Some(r) => r.clone(),
        None => rush_state::json::to_vec(input).unwrap_or_default(),
    };
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    if std::fs::metadata(&input.cwd).is_ok_and(|m| m.is_dir()) {
        c.current_dir(&input.cwd);
    }
    let out = crate::exec::run_with_stdin(c, raw, &Ctx::background(), Some(Duration::from_secs(2)));
    let s = String::from_utf8_lossy(&out).into_owned();
    s.trim_end_matches(['\n', ' ']).to_string()
}

/// The segment with id.
pub fn find(id: &str) -> Option<&'static Segment> {
    SEGMENTS.iter().find(|s| s.id == id)
}

/// Draws the lines: on each, the segments that have something to say,
/// joined. A segment that prints several lines (your own command) adds the
/// rest as lines of their own. What's slow (the branch, plan usage, the
/// account's name, your own command) is found out first, for the
/// segments shown.
pub fn render(input: &Input, l: &Layout, config_dir: &str, now: Time) -> String {
    let mut x = Extra {
        now,
        custom: l.custom.clone(),
        ..Default::default()
    };
    if l.shown("branch") {
        x.branch = git_branch(first_of(&[&input.workspace.current_dir, &input.cwd]));
    }
    if l.shown("usage") {
        x.usage = Some(plan_usage(config_dir));
    }
    if l.shown("account") {
        x.acct = account_name(config_dir);
    }
    if l.shown("health") {
        x.failing = failing();
    }
    if !l.custom.is_empty() && l.shown("custom") {
        x.own = run_own(&l.custom, input);
    }
    draw(input, l, &x)
}

/// Draws lines over and over, as the /statusline preview does many times
/// a second. It never waits: what's slow to find out (the branch, plan
/// usage, the account's name) it looks up off to the side, at most once a
/// second, and your own command runs off to the side too. Until they're
/// in, their segments show nothing, and then the last found.
#[derive(Default)]
pub struct Renderer(Arc<Mutex<RendererState>>);

#[derive(Default)]
struct RendererState {
    /// what was last looked up, for facts_dir and facts_cwd
    facts: Facts,
    facts_dir: String,
    facts_cwd: String,
    facts_at: Option<Instant>,
    looking: bool,
    own: String,
    own_key: String,
    own_at: Option<Instant>,
    running: bool,
}

/// What's slow to find out: git's branch, the plan's usage, the account's
/// name and the providers failing.
#[derive(Clone, Default)]
struct Facts {
    branch: String,
    usage: Option<Quota>,
    acct: String,
    failing: String,
}

impl Renderer {
    /// render, with what's slow looked up off to the side.
    pub fn render(&self, input: &Input, l: &Layout, config_dir: &str, now: Time) -> String {
        let f = {
            let mut r = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if r.facts_dir != config_dir || r.facts_cwd != input.cwd {
                r.facts = Facts::default();
                r.facts_dir = config_dir.into();
                r.facts_cwd = input.cwd.clone();
                r.facts_at = None;
            }
            if !r.looking
                && r.facts_at
                    .is_none_or(|at| at.elapsed() > Duration::from_secs(1))
            {
                r.looking = true;
                let me = self.0.clone();
                let (dir, cwd) = (config_dir.to_string(), input.cwd.clone());
                let at = first_of(&[&input.workspace.current_dir, &input.cwd]).to_string();
                std::thread::spawn(move || look_up(&me, &dir, &cwd, &at));
            }
            r.facts.clone()
        };
        let mut x = Extra {
            custom: l.custom.clone(),
            now,
            branch: f.branch,
            usage: f.usage,
            acct: f.acct,
            failing: f.failing,
            own: String::new(),
        };
        if !l.custom.is_empty() {
            x.own = self.own_line(&l.custom, input);
        }
        draw(input, l, &x)
    }

    /// Your own command's last output, starting it again when that is a
    /// couple of seconds old.
    fn own_line(&self, cmd: &str, input: &Input) -> String {
        let mut r = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if r.own_key != cmd {
            r.own = String::new();
            r.own_key = cmd.into();
            r.own_at = None;
        }
        if !r.running
            && r.own_at
                .is_none_or(|at| at.elapsed() > Duration::from_secs(2))
        {
            r.running = true;
            let me = self.0.clone();
            let (cmd, input) = (cmd.to_string(), input.clone());
            std::thread::spawn(move || {
                let out = run_own(&cmd, &input);
                let mut r = me.lock().unwrap_or_else(|e| e.into_inner());
                if r.own_key == cmd {
                    r.own = out;
                    r.own_at = Some(Instant::now());
                }
                r.running = false;
            });
        }
        r.own.clone()
    }
}

/// Finds the facts for config_dir and cwd, off the UI.
fn look_up(me: &Mutex<RendererState>, config_dir: &str, cwd: &str, dir: &str) {
    let f = Facts {
        branch: git_branch(dir),
        usage: Some(plan_usage(config_dir)),
        acct: account_name(config_dir),
        failing: failing(),
    };
    let mut r = me.lock().unwrap_or_else(|e| e.into_inner());
    r.looking = false;
    if r.facts_dir == config_dir && r.facts_cwd == cwd {
        r.facts = f;
        r.facts_at = Some(Instant::now());
    }
}

fn draw(input: &Input, l: &Layout, x: &Extra) -> String {
    let sep = if l.sep.is_empty() {
        default_layout().sep
    } else {
        l.sep.clone()
    };
    let mut out: Vec<String> = Vec::new();
    for ids in &l.lines {
        let (mut parts, mut more) = (Vec::new(), Vec::new());
        for id in ids {
            let Some(s) = find(id) else {
                continue;
            };
            let (text, col) = (s.draw)(input, x);
            if text.is_empty() {
                continue;
            }
            let (first, rest) = text.split_once('\n').unwrap_or((&text, ""));
            if !rest.is_empty() {
                more.extend(rest.split('\n').map(String::from));
            }
            if first.is_empty() {
                continue;
            }
            parts.push(if col.is_empty() {
                first.to_string()
            } else {
                format!("{col}{first}{RESET}")
            });
        }
        if !parts.is_empty() {
            out.push(parts.join(&format!("{DIM}{sep}{RESET}")));
        }
        out.extend(more);
    }
    let s = out.join("\n");
    if l.plain {
        return strip_sgr(&s);
    }
    s
}

static SGR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\x1b\\[[0-9;]*m").expect("a good pattern"));

fn strip_sgr(s: &str) -> String {
    SGR.replace_all(s, "").into_owned()
}

fn git_branch(dir: &str) -> String {
    if dir.is_empty() {
        return String::new();
    }
    match Command::new("git")
        .args(["-C", dir, "branch", "--show-current"])
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

fn logins_profile(dir: &str) -> rush_core::adapter::Profile {
    rush_core::adapter::Profile {
        kind: LOGINS_KIND.into(),
        dir: dir.into(),
        ..Default::default()
    }
}

/// The limits of the account the folder is signed in to, as rush last read
/// them.
fn plan_usage(config_dir: &str) -> Quota {
    rush_core::adapter::global()
        .get(LOGINS_KIND)
        .and_then(|a| {
            a.last_quota_reader()
                .and_then(|r| r.last_quota(&logins_profile(config_dir)))
        })
        .unwrap_or_default()
}

/// Each harness with a login failing, by name, and the error it last
/// failed on: "claude ✗ invalid_grant". A harness with several failing
/// shows the first's.
fn failing() -> String {
    let all = rush_state::health::all_failing();
    let mut parts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (k, e) in &all {
        let (h, _) = rush_state::health::split(k);
        if seen.insert(h.to_string()) {
            parts.push(format!("{h} ✗ {}", e.error));
        }
    }
    parts.join(" ")
}

/// The login the config folder at dir is signed in as.
fn account_name(dir: &str) -> String {
    let id = rush_core::adapter::global()
        .get(LOGINS_KIND)
        .and_then(|a| {
            a.sign_in_reader()
                .map(|r| r.signed_in_as(&logins_profile(dir)))
        })
        .unwrap_or_default();
    if !id.is_empty()
        && let Some(l) = rush_state::Store::load()
            .config
            .logins
            .iter()
            .find(|l| l.id == id)
    {
        return l.name.clone();
    }
    base(dir).trim_start_matches(".claude-").to_string()
}

/// A while in seconds, short: 1h05m, 12m, 40s.
fn short(secs: i64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, secs / 60 % 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn first_of<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// Where the layout is kept.
pub fn path() -> PathBuf {
    rush_state::dir().join("statusline.json")
}

/// Reads the layout, or the default.
pub fn load() -> Layout {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Saved {
        lines: Option<serde_json::Value>,
    }
    let Ok(b) = std::fs::read(path()) else {
        return default_layout();
    };
    let has_lines = rush_state::json::from_slice::<Saved>(&b)
        .is_ok_and(|s| s.lines.is_some_and(|l| !l.is_null()));
    match rush_state::json::from_slice::<Layout>(&b) {
        Ok(l) if has_lines => l,
        _ => default_layout(),
    }
}

/// Writes the layout.
pub fn save(l: &Layout) -> Result<(), String> {
    write_indented(&path(), l)
}

pub(crate) fn write_indented<T: Serialize>(p: &std::path::Path, v: &T) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut b = rush_state::json::to_vec_indent(v).map_err(|e| e.to_string())?;
    b.push(b'\n');
    if let Some(d) = p.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(d)
            .map_err(|e| e.to_string())?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)
        .and_then(|mut f| f.write_all(&b))
        .map_err(|e| e.to_string())
}

/// `rush statusline`: read the session from stdin, print the line.
pub fn run(mut stdin: impl Read, mut stdout: impl Write) -> Result<(), String> {
    let mut raw = Vec::new();
    stdin.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let mut input: Input = rush_state::json::from_slice(&raw).map_err(|e| e.to_string())?;
    input.raw = Some(raw);
    let dir = rush_core::adapter::global()
        .get(LOGINS_KIND)
        .and_then(|a| {
            a.status_line_profiler()
                .map(|p| p.status_line_profile().dir)
        })
        .unwrap_or_default();
    write!(stdout, "{}", render(&input, &load(), &dir, Time::now())).map_err(|e| e.to_string())
}

/// `rush statusline` as the binary runs it: prints "rush: <err>" and
/// returns 1 on an error, as Go's exitIf.
pub fn statusline_main() -> i32 {
    match run(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("rush: {e}");
            1
        }
    }
}

/// The statusLine command rush writes into settings.json: this rush as
/// installed (Go looked for rush on PATH first, statusline.go:565, which
/// can find an old Go rush still in ~/go/bin).
pub fn command() -> String {
    let bin = rush_sys::selfexe::resolved()
        .map_or_else(|| "rush".into(), |p| p.to_string_lossy().into_owned());
    format!("{} statusline", shell_quote(&bin))
}

/// Whether a statusLine command is rush's.
pub fn ours(cmd: &str) -> bool {
    let c = cmd.trim();
    c.ends_with("rush statusline") || c.ends_with("rush' statusline")
}

fn shell_quote(s: &str) -> String {
    if !s.contains([
        ' ', '\'', '"', '\\', '$', '`', '!', '*', '?', '[', ']', '(', ')', '{', '}', '<', '>', '|',
        '&', ';', '#', '~',
    ]) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;
