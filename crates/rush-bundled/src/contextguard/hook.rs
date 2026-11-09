//! The context guard's hook: given an agent's hook input (before a file
//! read or a shell call, after a read, an edit or a shell call, at a
//! compaction), it reads the file in question and what it keeps of the
//! session, asks the rules, keeps what changed, and says what the hook
//! answers. What it keeps is a file per agent session, under rush's own
//! folder, locked while it's read and written: an agent's hooks run at
//! once for tool calls made at once. Rust-only.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::images;
use super::rules::{self, FileStat, ImageStep, Outcome, Refusal, Rules, State};
use super::squeeze;

/// Files up to this size are read for their lines and a hash.
const HASHED: u64 = 256 * 1024;
/// Bytes a file's start is read for a NUL, to tell text.
const SNIFF: usize = 8 * 1024;
/// Squeezing that saves less than this isn't logged.
const LOG_SAVED: usize = 1024;
/// A session's file that hasn't changed in this long is let go.
const KEPT: Duration = Duration::from_secs(7 * 24 * 3600);

/// What the hook needs of the world.
pub struct Env {
    /// Where sessions' files are kept.
    pub dir: PathBuf,
    /// Where a cut output's whole is kept.
    pub tmp: PathBuf,
    pub rules: Rules,
    /// The time now as the agent's clock says it: 14:03:12.
    pub clock: String,
    /// And in seconds since 1970.
    pub now: u64,
    /// Claude Code's own folder (CLAUDE_CONFIG_DIR, else ~/.claude), for
    /// your permission rules about reads.
    pub claude_dir: Option<PathBuf>,
}

/// The hook's answer: what it prints, and what it logs (an event's kind
/// and fields), kind "" for nothing.
#[derive(Debug, PartialEq)]
pub struct Answer {
    pub out: Value,
    pub kind: &'static str,
    pub fields: Map<String, Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Input {
    hook_event_name: String,
    session_id: String,
    agent_id: String,
    tool_name: String,
    tool_input: Value,
    tool_response: Value,
    tool_use_id: String,
    error: String,
    is_interrupt: bool,
    source: String,
    cwd: String,
    permission_mode: String,
}

fn num(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A session's id as a file name, or "" when it can't be one.
fn file_name(id: &str) -> &str {
    if id.is_empty() || id.len() > 128 || id.contains(['/', '\\']) || id.starts_with('.') {
        return "";
    }
    id
}

/// What the guard makes of one hook input; None to say nothing.
pub fn step(input: &[u8], env: &Env) -> Option<Answer> {
    if !env.rules.any() {
        return None;
    }
    let inp: Input = serde_json::from_slice(input).ok()?;
    let session = file_name(&inp.session_id);
    if session.is_empty() {
        return None;
    }
    let agent = if inp.agent_id.is_empty() {
        "main"
    } else {
        inp.agent_id.as_str()
    };
    let path = || {
        let p = text(&inp.tool_input, "file_path");
        let p = if p.is_empty() {
            text(&inp.tool_input, "notebook_path")
        } else {
            p
        };
        if p.is_empty() || Path::new(p).is_absolute() || inp.cwd.is_empty() {
            p.to_string()
        } else {
            Path::new(&inp.cwd).join(p).to_string_lossy().into_owned()
        }
    };
    let tool = inp.tool_name.as_str();
    match (inp.hook_event_name.as_str(), tool) {
        ("SessionStart", _) => {
            if inp.source == "compact" || inp.source == "clear" {
                with_state(env, session, |st| {
                    if inp.source == "clear" {
                        st.agents.clear();
                    } else {
                        st.agents.values_mut().for_each(rules::forgot);
                    }
                });
            }
            None
        }
        ("PreCompact", _) => {
            with_state(env, session, |st| {
                if let Some(a) = st.agents.get_mut(agent) {
                    rules::forgot(a);
                }
            });
            None
        }
        ("PreToolUse" | "", "Read") => {
            let p = path();
            if images::is_image(Path::new(&p)) {
                return image_pre(&inp, env, session, agent, p);
            }
            if !env.rules.reread && !env.rules.big_read {
                return None;
            }
            let file = stat(Path::new(&p))?;
            let (offset, limit) = (
                num(&inp.tool_input, "offset"),
                num(&inp.tool_input, "limit"),
            );
            let r = with_state(env, session, |st| {
                rules::read(
                    st.agents.entry(agent.into()).or_default(),
                    env.rules,
                    &p,
                    offset,
                    limit,
                    &file,
                )
            })?;
            Some(deny(r, &inp, json!({"path": p})))
        }
        ("PreToolUse" | "", "Bash") => {
            if !env.rules.repeat {
                return None;
            }
            let command = text(&inp.tool_input, "command").to_string();
            let r = with_state(env, session, |st| {
                rules::bash(
                    st.agents.entry(agent.into()).or_default(),
                    env.rules,
                    &command,
                )
            })?;
            Some(deny(r, &inp, json!({"command": clip(&command)})))
        }
        ("PostToolUse", "Read") => {
            let p = path();
            if images::is_image(Path::new(&p)) {
                if env.rules.reread || env.rules.images {
                    with_state(env, session, |st| {
                        let a = st.agents.entry(agent.into()).or_default();
                        // Shown a copy, it looked at the image it asked for.
                        let asked = rules::image_asked(a, &inp.tool_use_id).unwrap_or(p);
                        let Some(file) = stat(Path::new(&asked)) else {
                            return;
                        };
                        let tokens = images::dimensions(Path::new(&asked))
                            .map_or(0, |(w, h)| images::billed(w, h));
                        rules::image_done(a, &asked, &inp.tool_use_id, file, tokens, &env.clock);
                    });
                }
                return None;
            }
            if !env.rules.reread {
                return None;
            }
            let file = stat(Path::new(&p))?;
            let (offset, limit) = (
                num(&inp.tool_input, "offset"),
                num(&inp.tool_input, "limit"),
            );
            with_state(env, session, |st| {
                rules::read_done(
                    st.agents.entry(agent.into()).or_default(),
                    &p,
                    offset,
                    limit,
                    file,
                    &env.clock,
                )
            });
            None
        }
        ("PostToolUse", "Edit" | "Write" | "MultiEdit" | "NotebookEdit") => {
            let p = path();
            with_state(env, session, |st| {
                if let Some(a) = st.agents.get_mut(agent) {
                    rules::edited(a, &p);
                }
            });
            None
        }
        ("PostToolUse", "Bash") => {
            let command = text(&inp.tool_input, "command").to_string();
            if env.rules.repeat {
                with_state(env, session, |st| {
                    if let Some(a) = st.agents.get_mut(agent) {
                        rules::bash_done(a, &command, Outcome::Ok);
                    }
                });
            }
            if !env.rules.squeeze || !squeeze::squeezes(&command) {
                return None;
            }
            squeezed(&inp, &command, &env.tmp, env.rules)
        }
        ("PostToolUseFailure", "Bash") => {
            if !env.rules.repeat {
                return None;
            }
            let command = text(&inp.tool_input, "command").to_string();
            let outcome = if inp.is_interrupt {
                Outcome::Interrupted
            } else {
                rules::failure(&inp.error)
            };
            with_state(env, session, |st| {
                rules::bash_done(
                    st.agents.entry(agent.into()).or_default(),
                    &command,
                    outcome,
                )
            });
            None
        }
        _ => None,
    }
}

/// Before a Read of an image: a re-read refused; else, where it helps, the
/// read pointed at a smaller WebP copy (Claude Code's updatedInput), and a
/// word on contact sheets once the agent has looked at many.
fn image_pre(inp: &Input, env: &Env, session: &str, agent: &str, p: String) -> Option<Answer> {
    let rules = env.rules;
    if !rules.reread && !rules.images {
        return None;
    }
    let src = Path::new(&p);
    let file = stat(src)?;
    let tokens = images::dimensions(src).map_or(0, |(w, h)| images::billed(w, h));
    let copy = if rules.images && may_rewrite(inp, env, &p) {
        images::shrink(src, &env.tmp.join(images::DIR), rules.image_tokens)
    } else {
        None
    };
    let step = with_state(env, session, |st| {
        Some(rules::image(
            st.agents.entry(agent.into()).or_default(),
            rules,
            &p,
            &file,
            tokens,
            copy.as_ref().map(|c| c.tokens().1),
            &inp.tool_use_id,
            env.now,
        ))
    })?;
    let (shrink, nudge) = match step {
        ImageStep::Refused(r) => return Some(deny(r, inp, json!({"path": p}))),
        ImageStep::Read { shrink, nudge } => (shrink, nudge),
    };
    let copy = copy.filter(|_| shrink);
    if copy.is_none() && nudge.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("hookEventName".into(), "PreToolUse".into());
    let mut fields = Map::new();
    fields.insert("tool".into(), Value::String(inp.tool_name.clone()));
    fields.insert("tool_use_id".into(), Value::String(inp.tool_use_id.clone()));
    if !inp.agent_id.is_empty() {
        fields.insert("agent".into(), Value::String(inp.agent_id.clone()));
    }
    fields.insert("path".into(), Value::String(p.clone()));
    let mut notes = Vec::new();
    if let Some(c) = &copy {
        let mut input = inp.tool_input.as_object().cloned().unwrap_or_default();
        input.insert(
            "file_path".into(),
            Value::String(c.path.to_string_lossy().into_owned()),
        );
        out.insert("permissionDecision".into(), "allow".into());
        out.insert(
            "permissionDecisionReason".into(),
            "context-guard: a smaller copy of the image".into(),
        );
        out.insert("updatedInput".into(), Value::Object(input));
        let (was, now) = c.tokens();
        let cost = if now < was {
            format!(
                "about {} tokens instead of {}",
                images::thousands(now),
                images::thousands(was)
            )
        } else {
            format!(
                "{} KB instead of {} KB",
                c.after.div_ceil(1024),
                c.before.div_ceil(1024)
            )
        };
        let why = format!(
            "context-guard: you're seeing a smaller copy of {p}: {}×{} WebP, from {}×{} ({cost}). If you need it \
             at full size, read {p} again straight away: that read goes through as it is.",
            c.to.0, c.to.1, c.from.0, c.from.1
        );
        fields.insert("why".into(), Value::String(why.clone()));
        fields.insert(
            "copy".into(),
            Value::String(c.path.to_string_lossy().into_owned()),
        );
        fields.insert(
            "from".into(),
            Value::String(format!("{}x{}", c.from.0, c.from.1)),
        );
        fields.insert("to".into(), Value::String(format!("{}x{}", c.to.0, c.to.1)));
        fields.insert("before".into(), Value::from(c.before));
        fields.insert("after".into(), Value::from(c.after));
        fields.insert("tokens_before".into(), Value::from(was));
        fields.insert("tokens_after".into(), Value::from(now));
        notes.push(why);
    }
    if !nudge.is_empty() {
        if copy.is_none() {
            fields.insert("why".into(), Value::String(nudge.clone()));
        }
        fields.insert("nudge".into(), Value::Bool(true));
        notes.push(nudge);
    }
    out.insert(
        "additionalContext".into(),
        Value::String(notes.join("\n\n")),
    );
    Some(Answer {
        out: json!({ "hookSpecificOutput": Value::Object(out) }),
        kind: if copy.is_some() {
            "hook-shrunk"
        } else {
            "hook-nudged"
        },
        fields,
    })
}

/// Whether the guard may point a read of p at a copy, which it does with
/// "allow": not when you have permission rules of your own about reads
/// (they'd be judged against the copy, not p), nor for a file Claude Code
/// would ask you about (outside the agent's folder and scratch folder,
/// unless it runs without asking).
fn may_rewrite(inp: &Input, env: &Env, p: &str) -> bool {
    if read_rules(env.claude_dir.as_deref(), &inp.cwd) {
        return false;
    }
    let inside = |dir: &str| !dir.is_empty() && Path::new(p).starts_with(dir);
    matches!(inp.permission_mode.as_str(), "bypassPermissions" | "auto")
        || inside(&inp.cwd)
        || inside(env.tmp.to_str().unwrap_or(""))
}

/// Whether your Claude Code settings (yours, the project's, the project's
/// own local) have a deny or ask rule about Read.
fn read_rules(claude_dir: Option<&Path>, cwd: &str) -> bool {
    let mut files = Vec::new();
    if let Some(d) = claude_dir {
        files.push(d.join("settings.json"));
    }
    if !cwd.is_empty() {
        let c = Path::new(cwd).join(".claude");
        files.push(c.join("settings.json"));
        files.push(c.join("settings.local.json"));
    }
    files.iter().any(|f| {
        let Some(v) = fs::read(f)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return false;
        };
        ["deny", "ask"].iter().any(|k| {
            v["permissions"][k].as_array().is_some_and(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .any(|r| r == "Read" || r.starts_with("Read("))
            })
        })
    })
}

fn clip(s: &str) -> String {
    match s.char_indices().nth(300) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A refusal as the hook says it, and as rush logs it.
fn deny(r: Refusal, inp: &Input, extra: Value) -> Answer {
    let mut fields = Map::new();
    fields.insert("why".into(), Value::String(r.why.clone()));
    fields.insert("rule".into(), Value::String(r.rule.into()));
    fields.insert("tool".into(), Value::String(inp.tool_name.clone()));
    fields.insert("tool_use_id".into(), Value::String(inp.tool_use_id.clone()));
    if !inp.agent_id.is_empty() {
        fields.insert("agent".into(), Value::String(inp.agent_id.clone()));
    }
    if let Value::Object(m) = extra {
        fields.extend(m);
    }
    Answer {
        out: json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": r.why,
        }}),
        kind: "hook-denied",
        fields,
    }
}

/// A shell call's output squeezed, as the hook replaces it; None when
/// squeezing changes nothing, or it isn't plain output.
fn squeezed(inp: &Input, command: &str, tmp: &Path, rules: Rules) -> Option<Answer> {
    let resp = inp.tool_response.as_object()?;
    let flag = |k: &str| resp.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("isImage")
        || resp.contains_key("persistedOutputPath")
        || resp.contains_key("backgroundTaskId")
    {
        return None;
    }
    let mut out = resp.clone();
    let (mut before, mut after) = (0, 0);
    let mut cut = false;
    for (key, suffix) in [("stdout", "out"), ("stderr", "err")] {
        let Some(s) = resp.get(key).and_then(Value::as_str) else {
            continue;
        };
        let mut q = squeeze::squeeze(s);
        if q.split_inclusive('\n').count() > rules.cap_lines {
            let id = if inp.tool_use_id.is_empty() || file_name(&inp.tool_use_id).is_empty() {
                format!("{:016x}", fnv(q.as_bytes()))
            } else {
                inp.tool_use_id.clone()
            };
            let kept = tmp.join("rush-output").join(format!("{id}.{suffix}.txt"));
            if keep(&kept, &q)
                && let Some(c) = squeeze::cap(
                    &q,
                    &kept.to_string_lossy(),
                    rules.cap_lines,
                    rules.keep_lines,
                )
            {
                q = c;
                cut = true;
            }
        }
        before += s.len();
        after += q.len();
        out.insert(key.into(), Value::String(q));
    }
    if after >= before {
        return None;
    }
    let saved = before - after;
    let mut fields = Map::new();
    if saved >= LOG_SAVED || cut {
        fields.insert("before".into(), Value::from(before));
        fields.insert("after".into(), Value::from(after));
        fields.insert("cut".into(), Value::Bool(cut));
        fields.insert("command".into(), Value::String(clip(command)));
        fields.insert("tool_use_id".into(), Value::String(inp.tool_use_id.clone()));
        fields.insert(
            "why".into(),
            Value::String(format!(
                "its output went from {before} to {after} bytes: terminal codes, progress redraws, spaces and repeated lines{}",
                if cut { ", and its middle, kept in a file" } else { "" }
            )),
        );
    }
    Some(Answer {
        out: json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": Value::Object(out),
        }}),
        kind: if fields.is_empty() {
            ""
        } else {
            "hook-squeezed"
        },
        fields,
    })
}

/// Writes text to path, for the agent alone.
fn keep(path: &Path, text: &str) -> bool {
    if let Some(d) = path.parent()
        && fs::create_dir_all(d).is_err()
    {
        return false;
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .is_ok()
}

/// FNV-1a: a hash that's the same in every build.
pub fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Not text, by its name: an image, a PDF, a notebook.
fn binary_kind(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "ico"
            | "tif"
            | "tiff"
            | "heic"
            | "pdf"
            | "ipynb"
    )
}

/// path as the disk has it now; None when it isn't a file.
pub fn stat(path: &Path) -> Option<FileStat> {
    let md = fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i128);
    let mut st = FileStat {
        size: md.len(),
        mtime,
        lines: None,
        hash: None,
        text: !binary_kind(path),
    };
    if !st.text {
        return Some(st);
    }
    let mut f = File::open(path).ok()?;
    if st.size <= HASHED {
        let mut b = Vec::with_capacity(st.size as usize);
        f.read_to_end(&mut b).ok()?;
        st.text = !b[..b.len().min(SNIFF)].contains(&0);
        let nl = b.iter().filter(|&&c| c == b'\n').count() as u64;
        st.lines = Some(nl + u64::from(!b.is_empty() && !b.ends_with(b"\n")));
        st.hash = Some(fnv(&b));
        st.size = b.len() as u64;
    } else {
        let mut b = vec![0; SNIFF];
        let n = f.read(&mut b).ok()?;
        st.text = !b[..n].contains(&0);
    }
    Some(st)
}

/// Runs f on the session's state, locked, and keeps what it changed.
/// Anything that goes wrong reading or writing leaves the state as it
/// was: the guard then says nothing rather than something wrong.
fn with_state<R: Default>(env: &Env, session: &str, f: impl FnOnce(&mut State) -> R) -> R {
    let dir = env.dir.join("context-guard");
    if fs::create_dir_all(&dir).is_err() {
        return R::default();
    }
    let Ok(lock) = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(format!("{session}.lock")))
    else {
        return R::default();
    };
    // SAFETY: flock on a descriptor lock owns; let go when it's closed.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return R::default();
    }
    let path = dir.join(format!("{session}.json"));
    let was = fs::read(&path).ok();
    if was.is_none() {
        tidy(&dir);
    }
    let mut st: State = was
        .as_deref()
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or_default();
    let before = st.clone();
    let r = f(&mut st);
    if st != before
        && let Ok(b) = serde_json::to_vec(&st)
    {
        let tmp = dir.join(format!(".{session}.json.tmp"));
        if fs::write(&tmp, b).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
    drop(lock);
    r
}

/// Lets go of sessions' files nothing has touched in a week.
fn tidy(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d > KEPT);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct T {
        _home: tempfile::TempDir,
        env: Env,
        work: PathBuf,
    }

    fn setup(rules: Rules) -> T {
        let home = tempfile::tempdir().unwrap();
        let work = home.path().join("work");
        fs::create_dir_all(&work).unwrap();
        T {
            env: Env {
                dir: home.path().join("state"),
                tmp: home.path().join("tmp"),
                rules,
                clock: "14:03:12".into(),
                now: 1_000_000,
                claude_dir: Some(home.path().join("claude")),
            },
            work,
            _home: home,
        }
    }

    impl T {
        /// The same folders, with other rules.
        fn with(&self, rules: Rules) -> T {
            T {
                _home: tempfile::tempdir().unwrap(),
                env: Env {
                    dir: self.env.dir.clone(),
                    tmp: self.env.tmp.clone(),
                    rules,
                    clock: self.env.clock.clone(),
                    now: self.env.now,
                    claude_dir: self.env.claude_dir.clone(),
                },
                work: self.work.clone(),
            }
        }
    }

    fn hook(t: &T, v: Value) -> Option<Answer> {
        step(v.to_string().as_bytes(), &t.env)
    }

    fn reason(a: &Option<Answer>) -> &str {
        a.as_ref()
            .and_then(|a| a.out["hookSpecificOutput"]["permissionDecisionReason"].as_str())
            .unwrap_or("")
    }

    fn read(t: &T, agent: &str, file: &str, event: &str) -> Option<Answer> {
        hook(
            t,
            json!({"hook_event_name": event, "session_id": "s1", "agent_id": agent, "tool_name": "Read",
                   "cwd": t.work, "tool_use_id": "toolu_r", "tool_input": {"file_path": file}}),
        )
    }

    /// The re-read story through the hook: per agent, reset by an edit, a
    /// change on disk and a compaction.
    #[test]
    fn rereads_through_the_hook() {
        let t = setup(Rules::ALL);
        let f = t.work.join("a.rs");
        fs::write(&f, "fn main() {}\n".repeat(400)).unwrap();
        let p = f.to_string_lossy().into_owned();
        assert_eq!(read(&t, "", &p, "PreToolUse"), None);
        assert_eq!(read(&t, "", &p, "PostToolUse"), None);
        let a = read(&t, "", "a.rs", "PreToolUse");
        assert!(
            reason(&a).contains("is unchanged since you read it at 14:03:12"),
            "{a:?}"
        );
        let a = a.unwrap();
        assert_eq!(a.kind, "hook-denied");
        assert_eq!(a.fields["rule"], "reread");
        assert_eq!(a.fields["path"], p.as_str());
        // Another agent hasn't read it.
        assert_eq!(read(&t, "sub1", &p, "PreToolUse"), None);
        // An edit lets it through.
        hook(
            &t,
            json!({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Edit",
                   "tool_input": {"file_path": p}}),
        );
        assert_eq!(read(&t, "", &p, "PreToolUse"), None);
        read(&t, "", &p, "PostToolUse");
        assert!(read(&t, "", &p, "PreToolUse").is_some());
        // So does a compaction.
        hook(
            &t,
            json!({"hook_event_name": "SessionStart", "session_id": "s1", "source": "compact"}),
        );
        assert_eq!(read(&t, "", &p, "PreToolUse"), None);
        read(&t, "", &p, "PostToolUse");
        // And a change on disk.
        fs::write(&f, "fn main() { }\n".repeat(400)).unwrap();
        assert_eq!(read(&t, "", &p, "PreToolUse"), None);
        // Off, it says nothing.
        read(&t, "", &p, "PostToolUse");
        assert!(read(&t, "", &p, "PreToolUse").is_some());
        let off = t.with(Rules {
            reread: false,
            ..Rules::ALL
        });
        assert_eq!(read(&off, "", &p, "PreToolUse"), None);
    }

    #[test]
    fn big_reads_through_the_hook() {
        let t = setup(Rules::ALL);
        let f = t.work.join("big.log");
        fs::write(&f, "a log line\n".repeat(3000)).unwrap();
        let p = f.to_string_lossy().into_owned();
        let a = read(&t, "", &p, "PreToolUse");
        assert!(reason(&a).contains("is 3000 lines, 33 KB"), "{a:?}");
        assert_eq!(read(&t, "", &p, "PreToolUse"), None, "asked again");
        let png = t.work.join("big.png");
        fs::write(&png, vec![b'x'; 100_000]).unwrap();
        assert_eq!(read(&t, "", &png.to_string_lossy(), "PreToolUse"), None);
    }

    fn bash(t: &T, event: &str, command: &str, extra: Value) -> Option<Answer> {
        let mut v = json!({"hook_event_name": event, "session_id": "s1", "tool_name": "Bash",
                           "tool_use_id": "toolu_b", "tool_input": {"command": command}});
        if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
            m.extend(e);
        }
        hook(t, v)
    }

    #[test]
    fn repeat_failures_through_the_hook() {
        let t = setup(Rules::ALL);
        let fail = json!({"error": "Exit code 1\nerror[E0425]: cannot find value `x`"});
        for _ in 0..3 {
            assert_eq!(bash(&t, "PreToolUse", "cargo build", json!({})), None);
            bash(&t, "PostToolUseFailure", "cargo build", fail.clone());
        }
        let a = bash(&t, "PreToolUse", "cargo  build", json!({}));
        assert!(
            reason(&a).contains("failed the same way 3 times in a row (exit 1"),
            "{a:?}"
        );
        assert_eq!(a.unwrap().fields["rule"], "repeat-fail");
        assert_eq!(
            bash(&t, "PreToolUse", "cargo build", json!({})),
            None,
            "insisted"
        );
    }

    #[test]
    fn squeezes_through_the_hook() {
        let t = setup(Rules::ALL);
        let out = "\x1b[32mok\x1b[0m   \n\n\n\ndone\n";
        let resp = json!({"tool_response": {"stdout": out, "stderr": "", "interrupted": false}});
        let a = bash(&t, "PostToolUse", "cargo build", resp.clone()).unwrap();
        assert_eq!(
            a.out,
            json!({"hookSpecificOutput": {"hookEventName": "PostToolUse",
                   "updatedToolOutput": {"stdout": "ok\n\ndone\n", "stderr": "", "interrupted": false}}})
        );
        assert_eq!(a.kind, "", "too little saved to log");
        // A file's own text is left as it is.
        assert_eq!(bash(&t, "PostToolUse", "cat a.rs", resp.clone()), None);
        // As is output already as small as it goes, and an image.
        let plain = json!({"tool_response": {"stdout": "ok\n", "stderr": ""}});
        assert_eq!(bash(&t, "PostToolUse", "make", plain), None);
        let img = json!({"tool_response": {"stdout": out, "isImage": true}});
        assert_eq!(bash(&t, "PostToolUse", "make", img), None);
        // A long one is cut, its whole kept for the agent.
        let long: String = (0..2000).map(|i| format!("test t{i} ... ok\n")).collect();
        let resp = json!({"tool_response": {"stdout": long, "stderr": ""}});
        let a = bash(&t, "PostToolUse", "cargo test", resp).unwrap();
        assert_eq!(a.kind, "hook-squeezed");
        let s = a.out["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .unwrap();
        let kept = t.env.tmp.join("rush-output").join("toolu_b.out.txt");
        assert!(
            s.contains(&format!(
                "… 1600 lines left out (the whole output: {}) …",
                kept.display()
            )),
            "{s}"
        );
        assert_eq!(fs::read_to_string(kept).unwrap(), long);
        // Off, nothing.
        let off = t.with(Rules {
            squeeze: false,
            ..Rules::ALL
        });
        let resp = json!({"tool_response": {"stdout": out, "stderr": ""}});
        assert_eq!(bash(&off, "PostToolUse", "cargo build", resp), None);
    }

    /// A screenshot-sized PNG with something in it.
    fn screenshot(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8]);
        }
        img.save(path).unwrap();
    }

    fn look(
        t: &T,
        session: &str,
        id: &str,
        file: &str,
        event: &str,
        extra: Value,
    ) -> Option<Answer> {
        let mut v = json!({"hook_event_name": event, "session_id": session, "tool_name": "Read",
                           "cwd": t.work, "tool_use_id": id, "tool_input": {"file_path": file}});
        if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
            m.extend(e);
        }
        hook(t, v)
    }

    fn context(a: &Option<Answer>) -> &str {
        a.as_ref()
            .and_then(|a| a.out["hookSpecificOutput"]["additionalContext"].as_str())
            .unwrap_or("")
    }

    /// An image read as a smaller copy; asked again straight after, the
    /// original; a third time, refused as unchanged.
    #[test]
    fn images_through_the_hook() {
        let t = setup(Rules::ALL);
        let f = t.work.join("shot.png");
        screenshot(&f, 3024, 1964);
        let p = f.to_string_lossy().into_owned();
        let a = look(&t, "s1", "toolu_1", &p, "PreToolUse", json!({})).unwrap();
        assert_eq!(a.kind, "hook-shrunk");
        let out = &a.out["hookSpecificOutput"];
        assert_eq!(out["permissionDecision"], "allow");
        let copy = out["updatedInput"]["file_path"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            copy.starts_with(&*t.env.tmp.join("rush-images").to_string_lossy()),
            "{copy}"
        );
        let (w, h) = images::dimensions(Path::new(&copy)).unwrap();
        assert!(images::tokens(w, h) <= 1568, "{w}x{h}");
        let said = out["additionalContext"].as_str().unwrap();
        assert!(
            said.starts_with(&format!("context-guard: you're seeing a smaller copy of {p}: {w}×{h} WebP, from 3024×1964 (about ")),
            "{said}"
        );
        assert!(
            said.contains("(about 1,568 tokens instead of 4,760)"),
            "{said}"
        );
        assert_eq!(
            (&a.fields["tokens_before"], &a.fields["tokens_after"]),
            (&json!(4760), &json!(1568))
        );
        // It went through, as the copy.
        look(&t, "s1", "toolu_1", &copy, "PostToolUse", json!({}));
        // Asked again straight after: the original, as it is.
        assert_eq!(look(&t, "s1", "toolu_2", &p, "PreToolUse", json!({})), None);
        look(&t, "s1", "toolu_2", &p, "PostToolUse", json!({}));
        // And again: it's in its context.
        let a = look(&t, "s1", "toolu_3", "shot.png", "PreToolUse", json!({}));
        assert!(
            reason(&a).starts_with(&format!(
                "context-guard: {p} is unchanged since you looked at it at 14:03:12. The image is still in your context"
            )),
            "{a:?}"
        );
        assert_eq!(a.unwrap().fields["rule"], "reread");
        // Changed, it's read again, as a copy.
        screenshot(&f, 3000, 2000);
        let a = look(&t, "s1", "toolu_4", &p, "PreToolUse", json!({}));
        assert_eq!(a.map(|a| a.kind), Some("hook-shrunk"));
        // Off, nothing: no copy, and no refusal with re-reads off too.
        let off = t.with(Rules {
            images: false,
            reread: false,
            ..Rules::ALL
        });
        assert_eq!(
            look(&off, "s2", "toolu_5", &p, "PreToolUse", json!({})),
            None
        );
    }

    #[test]
    fn images_left_alone() {
        let t = setup(Rules::ALL);
        let small = t.work.join("icon.png");
        screenshot(&small, 64, 64);
        let outside = tempfile::tempdir().unwrap();
        let far = outside.path().join("far.png");
        screenshot(&far, 2000, 1500);
        let far = far.to_string_lossy().into_owned();
        let small = small.to_string_lossy().into_owned();
        let cases: Vec<(&str, &str, Value, bool)> = vec![
            // Small already: nothing to say.
            ("small", &small, json!({}), false),
            // Outside the agent's folders, Claude Code would ask you: left to it.
            (
                "outside",
                &far,
                json!({"permission_mode": "default"}),
                false,
            ),
            // Unless it runs without asking.
            (
                "outside, auto",
                &far,
                json!({"permission_mode": "auto"}),
                true,
            ),
            (
                "outside, bypass",
                &far,
                json!({"permission_mode": "bypassPermissions"}),
                true,
            ),
        ];
        for (i, (what, p, extra, shrunk)) in cases.into_iter().enumerate() {
            let a = look(&t, &format!("s{i}"), "toolu_1", p, "PreToolUse", extra);
            assert_eq!(a.is_some_and(|a| a.kind == "hook-shrunk"), shrunk, "{what}");
        }
        // Your own rules about reads: left to them.
        let claude = t.env.claude_dir.clone().unwrap();
        fs::create_dir_all(&claude).unwrap();
        fs::write(
            claude.join("settings.json"),
            r#"{"permissions": {"deny": ["Read(./secrets/**)"]}}"#,
        )
        .unwrap();
        let a = look(
            &t,
            "s9",
            "toolu_1",
            &far,
            "PreToolUse",
            json!({"permission_mode": "auto"}),
        );
        assert_eq!(a, None);
    }

    /// Many images, or several at once, and the agent is pointed to a
    /// contact sheet: a word beside the read, never a refusal.
    #[test]
    fn contact_sheets_suggested() {
        let t = setup(Rules::ALL);
        let mut paths = Vec::new();
        for i in 0..10 {
            let f = t.work.join(format!("s{i}.png"));
            screenshot(&f, 120 + i, 90);
            paths.push(f.to_string_lossy().into_owned());
        }
        // One after another: the 9th gets the word.
        for (i, p) in paths.iter().enumerate().take(9) {
            let id = format!("toolu_{i}");
            let a = look(&t, "seq", &id, p, "PreToolUse", json!({}));
            if i < 8 {
                assert_eq!(a, None, "read {}", i + 1);
            } else {
                let a = a.unwrap();
                assert_eq!(a.kind, "hook-nudged");
                assert!(
                    a.out["hookSpecificOutput"]
                        .get("permissionDecision")
                        .is_none()
                );
                let said = a.out["hookSpecificOutput"]["additionalContext"]
                    .as_str()
                    .unwrap();
                assert!(
                    said.starts_with("context-guard: that's 9 images you've looked at in this conversation. Each one stays in your context"),
                    "{said}"
                );
                assert!(said.contains("mcp__rush__contact_sheet"), "{said}");
            }
            look(&t, "seq", &id, p, "PostToolUse", json!({}));
        }
        // Three at once: the third gets it, once.
        let at_once: Vec<Option<Answer>> = (0..4)
            .map(|i| {
                look(
                    &t,
                    "batch",
                    &format!("toolu_{i}"),
                    &paths[i],
                    "PreToolUse",
                    json!({}),
                )
            })
            .collect();
        assert_eq!(context(&at_once[0]), "");
        assert_eq!(context(&at_once[1]), "");
        assert!(
            context(&at_once[2]).starts_with("context-guard: that's 3 images read at once."),
            "{:?}",
            at_once[2]
        );
        assert_eq!(context(&at_once[3]), "");
        // Off, nothing.
        let off = t.with(Rules {
            images: false,
            ..Rules::ALL
        });
        for (i, p) in paths.iter().take(4).enumerate() {
            assert_eq!(
                look(
                    &off,
                    "off",
                    &format!("toolu_{i}"),
                    p,
                    "PreToolUse",
                    json!({})
                ),
                None
            );
        }
    }
}
