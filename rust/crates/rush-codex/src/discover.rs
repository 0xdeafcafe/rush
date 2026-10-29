//! Codex's threads outside rush: those whose rollouts it wrote to lately,
//! and every past one, each read from its rollout's head. Port of
//! internal/adapters/codex/discover.go:1-296.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rush_core::adapter::{Kind, Profile, Session};
use rush_core::time::{Ms, now};

use crate::KIND;
use crate::json::{Envelope, ms, parse, s as js, st};
use crate::rollout::{is_exec, is_subagent, read_lines, user_prompt};

/// How recently a rollout must have been written to for its thread to
/// count as running; WORKING_FOR, to count as working.
const LIVE_FOR: Ms = 2 * 60_000;
const WORKING_FOR: Ms = 15_000;

/// How many lines of a rollout Past reads at most looking for its first
/// prompt.
const HEAD_LINES: usize = 400;

/// How many characters of a prompt name a thread.
pub(crate) const NAME_LEN: usize = 80;

/// Rollouts' heads as last read, by path: a rollout is read again only
/// once it has changed, and one finished never does.
#[derive(Default)]
pub(crate) struct Heads(Mutex<HashMap<PathBuf, Head>>);

struct Head {
    modified: SystemTime,
    size: u64,
    s: Option<Session>,
}

fn mod_ms(t: SystemTime) -> Ms {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

/// The local calendar day `back` days before t: (year, month, day).
pub(crate) fn local_day(t: Ms, back: i64) -> (i32, u32, u32) {
    let secs = t.div_euclid(1000) as libc::time_t;
    // SAFETY: localtime_r and mktime only write the tm we hand them.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return (1970, 1, 1);
        }
        tm.tm_mday -= back as libc::c_int;
        tm.tm_hour = 12; // midday: no clock change moves it to another day
        tm.tm_isdst = -1;
        libc::mktime(&mut tm);
        (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
    }
}

/// The folder Codex files rollouts begun on a day in.
pub(crate) fn day_dir(home: &str, (y, m, d): (i32, u32, u32)) -> PathBuf {
    Path::new(home)
        .join("sessions")
        .join(format!("{y:04}"))
        .join(format!("{m:02}"))
        .join(format!("{d:02}"))
}

pub(crate) fn is_rollout(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// A folder's entries, by name.
fn entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map(|r| r.flatten().collect())
        .unwrap_or_default();
    out.sort_by_key(|e| e.file_name());
    out
}

impl Heads {
    /// The threads whose rollouts Codex wrote to lately: a codex in a
    /// terminal, most often. Codex keeps no record of which threads run
    /// that is cheap to read and can be trusted (its thread-writer-locks
    /// outlive their processes, and matching processes to rollouts would
    /// mean lsof on every refresh), so a rollout written in the last two
    /// minutes is taken as running, and one in the last fifteen seconds as
    /// working. Only the last week's folders are looked in: rollouts are
    /// filed by the day they began. Threads rush runs itself are also
    /// here; the host's rows claim them.
    pub fn live(&self, p: &Profile) -> Vec<Session> {
        let now = now();
        let mut names: Option<HashMap<String, String>> = None;
        let mut out = Vec::new();
        for d in 0..7 {
            let dir = day_dir(&p.dir, local_day(now, d));
            for e in entries(&dir) {
                let name = e.file_name().to_string_lossy().into_owned();
                if !is_rollout(&name) {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    continue;
                }
                let modified = md.modified().map_or(0, mod_ms);
                if now - modified > LIVE_FOR {
                    continue;
                }
                let Some(mut s) = self.past_session(&dir.join(&name)) else {
                    continue;
                };
                let names = names.get_or_insert_with(|| {
                    thread_names(&Path::new(&p.dir).join("session_index.jsonl"))
                });
                if let Some(n) = names.get(&s.id).filter(|n| !n.is_empty()) {
                    s.name = one_line(n);
                }
                s.kind = Kind::from(KIND);
                s.profile = p.clone();
                s.state = if now - modified < WORKING_FOR {
                    "working"
                } else {
                    "idle"
                }
                .into();
                out.push(s);
            }
        }
        out
    }

    /// Every rollout under the profile's sessions folder, newest first.
    /// Threads another thread started (spawned agents and the guardian
    /// reviewer) are left out: they belong to the thread that started
    /// them. A thread's name is the one Codex keeps in
    /// session_index.jsonl when it has one (only renamed or titled threads
    /// are there), else its first prompt.
    pub fn past(&self, p: &Profile) -> Vec<Session> {
        let names = thread_names(&Path::new(&p.dir).join("session_index.jsonl"));
        let mut out = Vec::new();
        self.walk(&Path::new(&p.dir).join("sessions"), &mut |path| {
            let Some(mut s) = self.past_session(path) else {
                return;
            };
            s.kind = Kind::from(KIND);
            s.profile = p.clone();
            s.state = "done".into();
            if let Some(n) = names.get(&s.id).filter(|n| !n.is_empty()) {
                s.name = one_line(n);
            }
            out.push(s);
        });
        out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        out
    }

    /// Calls f with each rollout under dir, in name order.
    fn walk(&self, dir: &Path, f: &mut dyn FnMut(&Path)) {
        for e in entries(dir) {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            if t.is_dir() {
                self.walk(&path, f);
            } else if is_rollout(&e.file_name().to_string_lossy()) {
                f(&path);
            }
        }
    }

    /// What a list needs from the head of a rollout. None for a subagent's
    /// rollout or one that can't be read.
    pub fn past_session(&self, path: &Path) -> Option<Session> {
        let md = std::fs::metadata(path).ok()?;
        let modified = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        {
            let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = m.get(path)
                && h.modified == modified
                && h.size == md.len()
            {
                return h.s.clone();
            }
        }
        let s = read_head(path, mod_ms(modified));
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.insert(
            path.to_path_buf(),
            Head {
                modified,
                size: md.len(),
                s: s.clone(),
            },
        );
        s
    }
}

fn read_head(path: &Path, modified: Ms) -> Option<Session> {
    let f = File::open(path).ok()?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let mut s = Session {
        id: id_from_name(&name.unwrap_or_default()),
        transcript: path.to_string_lossy().into_owned(),
        updated_at: modified,
        ..Session::default()
    };
    let (mut meta, mut sub, mut n) = (false, false, 0usize);
    let mut r = BufReader::with_capacity(64 << 10, f);
    let _ = read_lines(&mut r, |b, _| {
        n += 1;
        // Most lines are ones a head doesn't need, some of them big: those
        // are passed over on their type alone, never decoded.
        if n > 1 && !head_wants(b, meta, &s) {
            return n < HEAD_LINES && (s.name.is_empty() || s.model.is_empty());
        }
        let Ok(l) = serde_json::from_slice::<Envelope>(b) else {
            return n < HEAD_LINES;
        };
        if s.created_at == 0 {
            s.created_at = ms(&l.timestamp);
        }
        let payload = l.payload.map_or("null", |p| p.get());
        match &*l.typ {
            "session_meta" if !meta => {
                let m = parse(payload);
                if m.is_object() {
                    meta = true;
                    if is_subagent(&m) {
                        sub = true;
                        return false;
                    }
                    let id = js(&m, "id");
                    if !id.is_empty() {
                        s.id = id.to_string();
                    }
                    (s.cwd, s.headless) = (st(&m, "cwd"), is_exec(&m));
                    let t = ms(js(&m, "timestamp"));
                    if t != 0 {
                        s.created_at = t;
                    }
                }
            }
            "turn_context" if s.model.is_empty() => {
                s.model = st(&parse(payload), "model");
            }
            "response_item" if s.name.is_empty() => {
                let ri = parse(payload);
                if js(&ri, "type") == "message"
                    && js(&ri, "role") == "user"
                    && let Some(m) = user_prompt(&ri)
                {
                    for p in &m.parts {
                        if s.name.is_empty() && !p.text.trim().is_empty() {
                            s.name = one_line(&p.text);
                        }
                    }
                }
            }
            _ => {}
        }
        n < HEAD_LINES && (s.name.is_empty() || s.model.is_empty())
    });
    if sub {
        return None;
    }
    if s.updated_at == 0 {
        s.updated_at = s.created_at;
    }
    Some(s)
}

/// Whether read_head needs a rollout line, from its type: the session's
/// meta once, a turn's context until the model is known, and a user's
/// message until the session is named.
fn head_wants(b: &[u8], meta: bool, s: &Session) -> bool {
    // Codex writes the line's type before its payload; a line that isn't
    // written so is decoded to see.
    let head = &b[..b.len().min(160)];
    let i = memchr::memmem::find(head, br#""type":""#);
    let p = memchr::memmem::find(head, br#""payload":"#);
    let Some(i) = i else { return true };
    if p.is_some_and(|p| p < i) {
        return true;
    }
    let t = &b[i + 8..];
    if t.starts_with(br#"session_meta""#) {
        !meta
    } else if t.starts_with(br#"turn_context""#) {
        s.model.is_empty()
    } else if t.starts_with(br#"response_item""#) {
        s.name.is_empty() && memchr::memmem::find(b, br#""role":"user""#).is_some()
    } else {
        false
    }
}

/// The thread id at the end of a rollout's file name,
/// rollout-2026-09-22T17-01-09-<uuid>.jsonl.
pub(crate) fn id_from_name(name: &str) -> String {
    let name = name.strip_suffix(".jsonl").unwrap_or(name);
    if name.len() >= 36 && name.is_char_boundary(name.len() - 36) {
        return name[name.len() - 36..].to_string();
    }
    name.to_string()
}

/// session_index.jsonl: each named thread's latest name.
pub(crate) fn thread_names(path: &Path) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let Ok(f) = File::open(path) else {
        return names;
    };
    let _ = read_lines(&mut BufReader::new(f), |b, _| {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(b) {
            let (id, name) = (js(&v, "id"), js(&v, "thread_name"));
            if !id.is_empty() && !name.trim().is_empty() {
                names.insert(id.to_string(), name.to_string());
            }
        }
        true
    });
    names
}

/// text on one line, cut to NAME_LEN characters.
pub(crate) fn one_line(text: &str) -> String {
    // Only as much as could show is collapsed: a prompt can be a whole
    // pasted file.
    let mut b = String::new();
    let (mut n, mut gap) = (0usize, false);
    for c in text.chars() {
        if c.is_whitespace() {
            gap = !b.is_empty();
            continue;
        }
        if gap {
            b.push(' ');
            (n, gap) = (n + 1, false);
        }
        b.push(c);
        n += 1;
        if n > NAME_LEN {
            break;
        }
    }
    if b.chars().count() > NAME_LEN {
        let cut: String = b.chars().take(NAME_LEN - 1).collect();
        return cut.trim().to_string() + "…";
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rollout::tests::{Rollout, meta, t0, user_msg};
    use serde_json::json;

    fn set_mtime(path: &Path, t: SystemTime) {
        let f = File::options().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    // rollout_test.go:TestPast
    #[test]
    fn past_threads() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/09/20");
        let uuid = |n: u32| format!("01a0c9a2-0000-7000-8000-{n:012}");
        let path = |n: u32| day.join(format!("rollout-2026-09-20T10-00-0{n}-{}.jsonl", uuid(n)));
        let long = format!("please   look at\n\nthe {}long thing", "very ".repeat(30));
        Rollout::new(t0())
            .add("session_meta", meta(&uuid(1), "cli".into()))
            .add("turn_context", json!({"model": "gpt-5"}))
            .response(
                "message",
                user_msg(&["<environment_context>x</environment_context>"]),
            )
            .response("message", user_msg(&[&long]))
            .add(
                "response_item",
                json!({"type": "message", "role": "assistant", "content": []}),
            )
            .write(&path(1));
        Rollout::new(t0() + 3_600_000)
            .add("session_meta", meta(&uuid(2), "exec".into()))
            .add("turn_context", json!({"model": "gpt-5-mini"}))
            .response("message", user_msg(&["short one"]))
            .write(&path(2));
        // a spawned agent's thread
        Rollout::new(t0())
            .add(
                "session_meta",
                meta(&uuid(3), json!({"subagent": {"thread_spawn": {}}})),
            )
            .write(&path(3));
        // no session_meta: the id is the file name's
        Rollout::new(t0())
            .add("turn_context", json!({"model": "gpt-5"}))
            .response("message", user_msg(&["renamed"]))
            .write(&path(4));
        std::fs::write(day.join("notes.txt"), "x").unwrap();
        let index = format!(
            "{{\"id\":{:?},\"thread_name\":\"Old name\",\"updated_at\":\"2026-09-20T10:00:00Z\"}}\n{{\"id\":{:?},\"thread_name\":\"Fix the\\nlogin\",\"updated_at\":\"2026-09-20T11:00:00Z\"}}\n",
            uuid(4),
            uuid(4)
        );
        std::fs::write(dir.path().join("session_index.jsonl"), index).unwrap();
        let now = SystemTime::now();
        for (i, hours) in [(1, 3), (2, 2), (4, 1)] {
            set_mtime(&path(i), now - std::time::Duration::from_secs(hours * 3600));
        }

        let p = Profile {
            kind: Kind::from(KIND),
            name: ".codex".into(),
            dir: dir.path().to_string_lossy().into_owned(),
        };
        let h = Heads::default();
        let got = h.past(&p);
        let rows: Vec<(String, String, String, String, String, String)> = got
            .iter()
            .map(|s| {
                assert!(
                    s.kind.as_str() == KIND
                        && s.profile == p
                        && s.created_at != 0
                        && s.updated_at != 0,
                    "{s:?}"
                );
                (
                    s.id.clone(),
                    s.name.clone(),
                    s.model.clone(),
                    s.cwd.clone(),
                    s.state.clone(),
                    s.transcript.clone(),
                )
            })
            .collect();
        // 79 runes, cut at a space
        let want_name = format!("please look at the {}…", "very ".repeat(12).trim());
        let ps = |n| path(n).to_string_lossy().into_owned();
        let row = |a: String, b: &str, c: &str, d: &str, e: String| {
            (
                a,
                b.to_string(),
                c.to_string(),
                d.to_string(),
                "done".to_string(),
                e,
            )
        };
        assert_eq!(
            rows,
            vec![
                row(uuid(4), "Fix the login", "gpt-5", "", ps(4)),
                row(uuid(2), "short one", "gpt-5-mini", "/work", ps(2)),
                row(uuid(1), &want_name, "gpt-5", "/work", ps(1)),
            ]
        );
        // codex exec's, and a terminal's
        assert!(got[1].headless && !got[2].headless);
        assert_eq!(got[1].created_at, t0(), "the session_meta's");
        assert!(h.live(&p).is_empty());
        // Read again, the heads are as kept.
        assert_eq!(h.past(&p).len(), 3);
    }

    // rollout_test.go:TestLiveIsWhatWasWrittenLately
    #[test]
    fn live_is_what_was_written_lately() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let day = day_dir(&home, local_day(now(), 0));
        std::fs::create_dir_all(&day).unwrap();
        let now = SystemTime::now();
        let write = |id: &str, age: u64| {
            let line = format!(
                r#"{{"timestamp":"2026-09-27T10:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"/w"}}}}"#
            );
            let path = day.join(format!("rollout-2026-09-27T10-00-00-{id}.jsonl"));
            std::fs::write(&path, line + "\n").unwrap();
            set_mtime(&path, now - std::time::Duration::from_secs(age));
        };
        write("0199aaaa-0000-7000-8000-000000000001", 5);
        write("0199aaaa-0000-7000-8000-000000000002", 60);
        write("0199aaaa-0000-7000-8000-000000000003", 3600);
        let live = Heads::default().live(&Profile {
            dir: home,
            ..Profile::default()
        });
        let states: HashMap<String, String> = live
            .iter()
            .map(|s| (s.id[s.id.len() - 1..].to_string(), s.state.clone()))
            .collect();
        assert_eq!(live.len(), 2, "{states:?}");
        assert_eq!(states["1"], "working");
        assert_eq!(states["2"], "idle");
    }

    // rollout_test.go:TestOneLineMatchesWhole: one_line collapses only the
    // start of a prompt, and names it just as collapsing the whole of it
    // would.
    #[test]
    fn one_line_matches_whole() {
        let whole = |text: &str| {
            let s = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if s.chars().count() > NAME_LEN {
                let cut: String = s.chars().take(NAME_LEN - 1).collect();
                return cut.trim().to_string() + "…";
            }
            s
        };
        let long = format!("long-word-{}", "y".repeat(70));
        let words = [
            "fix",
            "the",
            " ",
            "\n\n",
            "\t",
            "é",
            "日本語",
            "x",
            "  very  ",
            &long,
        ];
        for i in 0..2000 {
            let mut b = String::new();
            for j in 0..i % 60 {
                b += words[(i * 7 + j * 13) % words.len()];
            }
            assert_eq!(one_line(&b), whole(&b), "{b:?}");
        }
    }

    #[test]
    fn ids_and_days() {
        assert_eq!(
            id_from_name("rollout-2026-09-22T17-01-09-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000.jsonl"),
            "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000"
        );
        assert_eq!(id_from_name("short.jsonl"), "short");
        let (y, m, d) = local_day(now(), 0);
        assert!(y >= 2026 && (1..=12).contains(&m) && (1..=31).contains(&d));
        let (_, _, d1) = local_day(now(), 1);
        assert_ne!(d, d1);
    }
}
