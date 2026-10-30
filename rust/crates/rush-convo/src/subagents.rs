//! A session's subagent runs as Claude Code records them beside its
//! transcript: `<session>/subagents/agent-<id>.jsonl` and its meta, a
//! workflow's a folder down in `workflows/wf_<run>/`. Port of
//! internal/convo/subagents.go (Subagent :17, Subagents.List :55,
//! SubagentTail :135).
//!
//! Listing reads a run's meta only when it's new or changed, and the
//! folder only when something was added to it; it's done off the UI
//! thread by whoever lists (rush-ui's session_core::subs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rush_core::time::Ms;

/// One run of a subagent (convo.Subagent).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subagent {
    pub id: String,
    pub typ: String,
    pub description: String,
    pub model: String,
    pub tool_use_id: String,
    /// 1 for one the session started, more for one a run started.
    pub depth: i64,
    /// The workflow run that started it, if one did.
    pub workflow: String,
    /// The workflow's phase it ran in.
    pub phase: String,
    /// Its own transcript.
    pub path: String,
    /// When it started: its meta file's time as first seen.
    pub born: Ms,
    /// born to the nanosecond, when known (0 when not): Go keeps Born in
    /// nanoseconds and sorts by it (subagents.go:122), so two runs begun
    /// in the same millisecond keep Go's order.
    pub born_ns: i64,
    /// When it last wrote.
    pub modified: Ms,
    /// Its transcript's size, to skip reading one that hasn't grown.
    pub size: u64,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Meta {
    #[serde(rename = "agentType")]
    agent_type: String,
    description: String,
    model: String,
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    #[serde(rename = "spawnDepth")]
    spawn_depth: i64,
    #[serde(rename = "workflowPhase")]
    workflow_phase: String,
}

#[derive(Clone, Default)]
struct SubMeta {
    modified: Option<SystemTime>,
    size: u64,
    sa: Subagent,
    ok: bool,
}

/// Lists a session's subagents again and again (convo.Subagents).
#[derive(Clone, Default)]
pub struct Subagents {
    metas: HashMap<PathBuf, SubMeta>,
    dir: PathBuf,
    dir_mod: Option<SystemTime>,
    run_mods: HashMap<PathBuf, SystemTime>,
    names: Vec<PathBuf>,
    scanned: Option<SystemTime>,
}

fn ms(t: SystemTime) -> Ms {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Ms)
}

fn ns(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

impl Subagent {
    /// When it began, in nanoseconds: born_ns when known, else its
    /// millisecond. What Go's Born orders by.
    pub fn born_order(&self) -> i128 {
        if self.born_ns != 0 {
            i128::from(self.born_ns)
        } else {
            i128::from(self.born) * 1_000_000
        }
    }
}

fn glob(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with(prefix) && n.ends_with(suffix)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The subagents of the session whose transcript is at path, oldest
/// first (ListSubagents).
pub fn list_subagents(transcript: &str) -> Vec<Subagent> {
    Subagents::default().list(transcript)
}

impl Subagents {
    /// The runs beside transcript, by when each began (List).
    pub fn list(&mut self, transcript: &str) -> Vec<Subagent> {
        let dir =
            Path::new(transcript.strip_suffix(".jsonl").unwrap_or(transcript)).join("subagents");
        if self.dir != dir {
            *self = Subagents {
                dir: dir.clone(),
                ..Subagents::default()
            };
        }
        let Ok(st) = std::fs::metadata(&dir) else {
            self.names.clear();
            self.dir_mod = None;
            return Vec::new();
        };
        let dir_mod = st.modified().ok();
        // A workflow's agents are added to its own folder.
        let runs = glob(&dir.join("workflows"), "wf_", "");
        let run_mods: HashMap<PathBuf, SystemTime> = runs
            .iter()
            .filter_map(|r| Some((r.clone(), std::fs::metadata(r).ok()?.modified().ok()?)))
            .collect();
        // A new run adds files to the folder, which changes its time; the
        // metas are read again at least every 10s in case one was rewritten.
        let fresh = self
            .scanned
            .and_then(|s| s.elapsed().ok())
            .is_some_and(|e| e.as_secs() < 10)
            && dir_mod == self.dir_mod
            && run_mods == self.run_mods;
        if !fresh {
            self.names = glob(&dir, "agent-", ".meta.json");
            for r in &runs {
                self.names.extend(glob(r, "agent-", ".meta.json"));
            }
            self.dir_mod = dir_mod;
            self.run_mods = run_mods;
            self.scanned = Some(SystemTime::now());
        }
        let workflows = dir.join("workflows");
        let mut out = Vec::with_capacity(self.names.len());
        for meta in &self.names {
            let seen = self.metas.get(meta).cloned();
            let mut m = seen.clone().unwrap_or_default();
            if seen.is_none() || !fresh || !m.ok {
                let Ok(st) = std::fs::metadata(meta) else {
                    continue;
                };
                let modified = st.modified().ok();
                if seen.is_none() || m.modified != modified || m.size != st.len() {
                    // A rewrite doesn't make it any younger.
                    let (born, born_ns) = (m.sa.born, m.sa.born_ns);
                    m = SubMeta {
                        modified,
                        size: st.len(),
                        ..SubMeta::default()
                    };
                    (m.sa.born, m.sa.born_ns) = if born == 0 && born_ns == 0 {
                        (modified.map_or(0, ms), modified.map_or(0, ns))
                    } else {
                        (born, born_ns)
                    };
                    if let Some(j) = std::fs::read(meta)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
                    {
                        m.ok = true;
                        let name = meta.file_name().unwrap_or_default().to_string_lossy();
                        let id = name
                            .trim_start_matches("agent-")
                            .trim_end_matches(".meta.json")
                            .to_string();
                        let parent = meta.parent().unwrap_or(Path::new(""));
                        m.sa.path = parent
                            .join(format!("agent-{id}.jsonl"))
                            .to_string_lossy()
                            .into_owned();
                        m.sa.id = id;
                        m.sa.typ = j.agent_type;
                        m.sa.description = j.description;
                        m.sa.model = j.model;
                        m.sa.tool_use_id = j.tool_use_id;
                        m.sa.depth = j.spawn_depth;
                        m.sa.phase = j.workflow_phase;
                        if parent.parent() == Some(workflows.as_path()) {
                            m.sa.workflow = parent
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                        }
                    }
                    // A half-written one changes size, and is read again.
                    self.metas.insert(meta.clone(), m.clone());
                }
            }
            if !m.ok {
                continue;
            }
            let mut sa = m.sa;
            if let Ok(st) = std::fs::metadata(&sa.path) {
                sa.modified = st.modified().map_or(0, ms);
                sa.size = st.len();
            }
            out.push(sa);
        }
        // By when each began, not last wrote: rows stay put as runs write.
        // To the nanosecond, as Go: runs begun in one millisecond keep
        // the order they began in (subagents.go:120).
        out.sort_by(|a, b| {
            a.born_order()
                .cmp(&b.born_order())
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    // subagents_test.go: a session's runs, and a workflow's a folder down,
    // read from their metas; a half-written meta waits.
    #[test]
    fn lists_runs_and_workflow_agents() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s1.jsonl");
        write(&t, "");
        let sub = d.path().join("s1/subagents");
        write(
            &sub.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"find the bug","toolUseId":"toolu_1","spawnDepth":1}"#,
        );
        write(&sub.join("agent-a1.jsonl"), "{}\n");
        write(
            &sub.join("workflows/wf_9/agent-b2.meta.json"),
            r#"{"agentType":"workflow","description":"port:draw","workflowPhase":"Port"}"#,
        );
        write(&sub.join("agent-c3.meta.json"), r#"{"agentType":"#);
        let got = list_subagents(&t.to_string_lossy());
        let ids: Vec<_> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "{got:?}");
        let a = got.iter().find(|s| s.id == "a1").unwrap();
        assert_eq!(
            (
                a.typ.as_str(),
                a.description.as_str(),
                a.tool_use_id.as_str(),
                a.depth,
                a.size
            ),
            ("Explore", "find the bug", "toolu_1", 1, 3)
        );
        assert!(a.path.ends_with("subagents/agent-a1.jsonl"));
        let b = got.iter().find(|s| s.id == "b2").unwrap();
        assert_eq!((b.workflow.as_str(), b.phase.as_str()), ("wf_9", "Port"));
        // None beside a transcript with no folder.
        assert!(list_subagents(&d.path().join("none.jsonl").to_string_lossy()).is_empty());
    }

    // subagents.go:120: runs are ordered by when each began to the
    // nanosecond, so two begun in one millisecond keep their order rather
    // than falling back to their ids.
    #[test]
    fn same_millisecond_runs_keep_the_order_they_began() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s1.jsonl");
        write(&t, "");
        let sub = d.path().join("s1/subagents");
        let base =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(1_700_000_000_123_000_100);
        for (id, off) in [("z1", 0u64), ("a1", 400)] {
            let p = sub.join(format!("agent-{id}.meta.json"));
            write(&p, r#"{"agentType":"Explore"}"#);
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(base + std::time::Duration::from_nanos(off))
                .unwrap();
        }
        let got = list_subagents(&t.to_string_lossy());
        let ids: Vec<_> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["z1", "a1"]);
        assert_eq!(got[0].born, got[1].born, "one millisecond");
        assert_eq!(got[1].born_ns - got[0].born_ns, 400);
    }
}
