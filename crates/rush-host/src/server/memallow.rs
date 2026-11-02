//! One answer for a memory. Allowing (or declining) the agent's write of
//! one of its memory notes answers the writes that carry that memory out
//! the same way, without asking again: the note itself once more, and its
//! line in the index, MEMORY.md. Only files directly in the session's own
//! memory folder, which its harness names (MemoryReader::memory_dir), only
//! in the same turn and for MEM_WINDOW. Each is logged as permission-auto,
//! reason memory, so `rush why` says why nobody was asked.
//!
//! Rust-only: Go asked for each write.

use std::path::Path;
use std::time::{Duration, Instant};

use rush_core::adapter::Profile;
use rush_core::event;
use rush_core::tool::{Call, Kind};
use serde_json::json;

use super::{Host, State};
use crate::op::Op;

/// How long after its answer a memory's other writes are answered too.
pub(super) const MEM_WINDOW: Duration = Duration::from_secs(120);

/// The index of a memory folder.
const INDEX: &str = "MEMORY.md";

/// The refusal a write gets when the memory it carries out was declined.
pub(super) const MEM_DECLINED: &str =
    "Declined: the user declined the memory this write goes with. Don't save it.";

/// How the user answered a write into the memory folder.
#[derive(Clone, Debug)]
pub(super) struct MemAnswer {
    /// The approval answered.
    pub id: String,
    pub dir: String,
    pub path: String,
    pub allow: bool,
    pub at: Instant,
}

/// The file call writes, when it's a Write or Edit of one.
fn written(call: &Call) -> Option<&str> {
    matches!(call.kind, Kind::Write | Kind::Edit)
        .then_some(call.input.path.as_str())
        .filter(|p| !p.is_empty())
}

fn is_index(path: &str) -> bool {
    Path::new(path).file_name().is_some_and(|n| n == INDEX)
}

/// Whether path is a markdown file directly in dir: as written, or once
/// links are followed.
pub(super) fn in_dir(dir: &str, path: &str) -> bool {
    let p = Path::new(path);
    if dir.is_empty() || !p.is_absolute() || p.extension().is_none_or(|e| e != "md") {
        return false;
    }
    let Some(parent) = p.parent() else {
        return false;
    };
    if parent.components().any(|c| c.as_os_str() == "..") {
        return false;
    }
    let dir = Path::new(dir);
    parent == dir
        || matches!(
            (parent.canonicalize(), dir.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

impl MemAnswer {
    /// Whether call, asked at now, is one of the writes this answer
    /// carries: the same note, or the index (any note, when it was the
    /// index that was answered), in the same folder, within MEM_WINDOW.
    pub(super) fn covers(&self, call: &Call, now: Instant) -> bool {
        if now.saturating_duration_since(self.at) > MEM_WINDOW {
            return false;
        }
        let Some(p) = written(call) else {
            return false;
        };
        in_dir(&self.dir, p)
            && (Path::new(p) == Path::new(&self.path) || is_index(p) || is_index(&self.path))
    }
}

impl Host {
    /// Answers approval a as the memory it carries out was answered, when
    /// it's one of that memory's writes, and says whether it did. The
    /// answer goes off the lock, as the user's would.
    pub(super) fn answer_as_memory(&self, st: &mut State, a: &event::Approval) -> bool {
        let Some(m) = st
            .mem_answer
            .as_ref()
            .filter(|m| m.covers(&a.call, Instant::now()))
        else {
            return false;
        };
        let allow = m.allow;
        self.event(
            st,
            "permission-auto",
            json!({
                "reason": "memory",
                "answer": if allow { "allow" } else { "deny" },
                "tool": a.call.name,
                "path": a.call.input.path,
                "id": a.id,
                "as": m.id,
                "note": m.path,
            }),
        );
        let (h, id) = (self.clone(), a.id.clone());
        self.go("memory-answer", move |_| {
            let mut o = Op::named(if allow { "allow" } else { "deny" });
            o.id = id;
            if !allow {
                o.message = MEM_DECLINED.into();
            }
            let _ = h.do_op(o);
        });
        true
    }

    /// Keeps how the user answered call (approval id), when it writes into
    /// the session's memory folder, for that memory's other writes. Called
    /// off the lock, before the answer reaches the agent: its next ask may
    /// follow at once.
    pub(super) fn note_mem_answer(&self, id: &str, call: &Call, allow: bool) {
        let Some(path) = written(call) else {
            return;
        };
        if !path.ends_with(".md") {
            return;
        }
        let (kind, p, cwd): (String, Profile, String) = {
            let st = self.lock();
            if st
                .mem_answer
                .as_ref()
                .is_some_and(|m| m.covers(call, Instant::now()))
            {
                return; // one of its own writes: it doesn't widen it
            }
            (
                st.cfg.kind.clone(),
                st.cfg.account.clone(),
                st.cfg.cwd.clone(),
            )
        };
        let dir = self
            .reg
            .get(&kind)
            .and_then(|a| a.memory_reader().map(|r| r.memory_dir(&p, &cwd)))
            .unwrap_or_default();
        if !in_dir(&dir, path) {
            return;
        }
        self.lock().mem_answer = Some(MemAnswer {
            id: id.to_string(),
            dir,
            path: path.to_string(),
            allow,
            at: Instant::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};

    use rush_core::adapter::{
        self, Adapter, Conn, Features, Input, Level, Memory, MemoryReader, Registry,
    };
    use rush_core::event::{ApprovalOption, Event, OptionKind};
    use rush_core::tool;

    use super::super::tests::with_conn;
    use super::super::{Hooks, build};
    use super::*;
    use crate::paths::dir;
    use crate::testing::Home;
    use crate::wire::Config;

    /// An agent whose memory notes are kept in one folder.
    struct Remembers(String);

    impl Adapter for Remembers {
        fn kind(&self) -> adapter::Kind {
            adapter::Kind::from("remembers")
        }
        fn name(&self) -> String {
            "Remembers".into()
        }
        fn features(&self) -> Features {
            Features::default()
        }
        fn level(&self) -> Level {
            Level::Preview
        }
        fn profiles(&self) -> Vec<Profile> {
            Vec::new()
        }
        rush_core::has!(memory_reader: MemoryReader);
    }

    impl MemoryReader for Remembers {
        fn memory(&self, _: &Profile, _: &str, _: &str) -> Memory {
            Memory {
                dir: self.0.clone(),
                ..Memory::default()
            }
        }
        fn forget_file(&self, _: &str) -> adapter::Result<()> {
            Ok(())
        }
    }

    /// A conn that says each answer it's given: (approval, option).
    struct Answers(Mutex<Sender<(String, String)>>);

    impl Conn for Answers {
        fn send(&self, _: Input) -> adapter::Result<()> {
            Ok(())
        }
        fn answer(&self, id: &str, option: &str) -> adapter::Result<()> {
            let _ = self.0.lock().unwrap().send((id.into(), option.into()));
            Ok(())
        }
        fn interrupt(&self) -> adapter::Result<()> {
            Ok(())
        }
        fn set_model(&self, _: &str) -> adapter::Result<()> {
            Ok(())
        }
        fn set_mode(&self, _: &str) -> adapter::Result<()> {
            Ok(())
        }
        fn close(&self) -> adapter::Result<()> {
            Ok(())
        }
    }

    /// A host of the agent that remembers into mem, and what its agent is
    /// answered.
    fn remembering(id: &str, mem: &str) -> (Host, Arc<dyn Conn>, Receiver<(String, String)>) {
        let reg = Registry::new();
        reg.register(Remembers(mem.into()));
        std::fs::create_dir_all(dir(id)).unwrap();
        let (h, _rx) = build(
            reg,
            Hooks::quiet(),
            Config {
                id: id.into(),
                kind: "remembers".into(),
                ..Config::default()
            },
        );
        let (tx, rx) = channel();
        let conn: Arc<dyn Conn> = Arc::new(Answers(Mutex::new(tx)));
        with_conn(&h, conn.clone());
        h.lock().info.state = "working".into();
        (h, conn, rx)
    }

    /// The agent asks to write path (a Write, or an Edit).
    fn asks(h: &Host, conn: &Arc<dyn Conn>, id: &str, kind: tool::Kind, path: &str) {
        let ap = event::Approval {
            id: id.into(),
            call: tool::Call {
                name: format!("{kind:?}"),
                kind,
                input: tool::Input {
                    path: path.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            options: vec![
                ApprovalOption {
                    id: "yes".into(),
                    kind: OptionKind::AllowOnce,
                    ..Default::default()
                },
                ApprovalOption {
                    id: "no".into(),
                    kind: OptionKind::RejectOnce,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut st = h.lock();
        st.info.state = "working".into(); // blocked again only if it asks
        h.on_agent_event(&mut st, conn, Event::Approval(Box::new(ap)));
    }

    fn answer(h: &Host, id: &str, allow: bool) {
        let mut o = Op::named(if allow { "allow" } else { "deny" });
        o.id = id.into();
        h.do_op(o).unwrap();
    }

    fn got(rx: &Receiver<(String, String)>) -> (String, String) {
        rx.recv_timeout(Duration::from_secs(10)).expect("an answer")
    }

    /// Whether approval id still waits on the user.
    fn asked(h: &Host, id: &str) -> bool {
        let st = h.lock();
        st.pending.contains_key(id) && st.info.state == "blocked"
    }

    #[test]
    fn allowing_a_memory_allows_its_writes() {
        let home = Home::new();
        let mem = home.path().join("projects/p/memory");
        let mem = mem.to_string_lossy().into_owned();
        let (h, conn, rx) = remembering("mema", &mem);
        let note = format!("{mem}/feedback_tests.md");
        asks(&h, &conn, "a1", tool::Kind::Write, &note);
        assert!(asked(&h, "a1"), "the memory itself is asked");
        answer(&h, "a1", true);
        assert_eq!(got(&rx), ("a1".into(), "yes".into()));
        // Its index line goes without a second card.
        asks(
            &h,
            &conn,
            "a2",
            tool::Kind::Edit,
            &format!("{mem}/MEMORY.md"),
        );
        assert_eq!(got(&rx), ("a2".into(), "yes".into()));
        // A write anywhere else still asks: another folder, another note,
        // or a file beside the folder.
        for (id, p) in [
            ("o1", "/tmp/elsewhere/MEMORY.md".to_string()),
            ("o2", format!("{mem}/other_note.md")),
            ("o3", format!("{mem}/../MEMORY.md")),
            ("o4", format!("{mem}/x/MEMORY.md")),
        ] {
            asks(&h, &conn, id, tool::Kind::Write, &p);
            assert!(asked(&h, id), "{p} wasn't asked");
            assert!(rx.try_recv().is_err(), "{p} was answered");
        }
        // Nor does a shell command in the folder go.
        asks(&h, &conn, "o5", tool::Kind::Shell, &note);
        assert!(asked(&h, "o5"));
        // Long after, the memory's writes are asked again.
        h.lock().mem_answer.as_mut().unwrap().at -= MEM_WINDOW * 2;
        asks(&h, &conn, "a3", tool::Kind::Edit, &note);
        assert!(asked(&h, "a3"), "a memory long answered still answers");
        crate::events::flush();
        let ev = std::fs::read_to_string(crate::paths::events_path("mema")).unwrap_or_default();
        assert!(
            ev.lines().any(|l| l.contains("\"permission-auto\"")
                && l.contains("\"reason\":\"memory\"")
                && l.contains("MEMORY.md")),
            "{ev}"
        );
    }

    #[test]
    fn declining_a_memory_declines_its_writes() {
        let home = Home::new();
        let mem = home.path().join("projects/p/memory");
        let mem = mem.to_string_lossy().into_owned();
        let (h, conn, rx) = remembering("memd", &mem);
        asks(
            &h,
            &conn,
            "d1",
            tool::Kind::Write,
            &format!("{mem}/user_role.md"),
        );
        answer(&h, "d1", false);
        assert_eq!(got(&rx), ("d1".into(), "no".into()));
        asks(
            &h,
            &conn,
            "d2",
            tool::Kind::Edit,
            &format!("{mem}/MEMORY.md"),
        );
        assert_eq!(got(&rx), ("d2".into(), "no".into()));
    }

    #[test]
    fn a_write_outside_the_memory_folder_is_never_remembered() {
        let home = Home::new();
        let mem = home.path().join("projects/p/memory");
        let mem = mem.to_string_lossy().into_owned();
        let (h, conn, rx) = remembering("memo", &mem);
        asks(&h, &conn, "w1", tool::Kind::Write, "/tmp/src/notes.md");
        answer(&h, "w1", true);
        assert_eq!(got(&rx), ("w1".into(), "yes".into()));
        assert!(h.lock().mem_answer.is_none());
        asks(
            &h,
            &conn,
            "w2",
            tool::Kind::Edit,
            &format!("{mem}/MEMORY.md"),
        );
        assert!(asked(&h, "w2"));
    }

    #[test]
    fn memory_folder_paths() {
        for (path, want) in [
            ("/m/memory/a.md", true),
            ("/m/memory/MEMORY.md", true),
            ("/m/memory/a.txt", false),
            ("/m/memory/sub/a.md", false),
            ("/m/memory/../a.md", false),
            ("memory/a.md", false),
            ("/m/other/a.md", false),
        ] {
            assert_eq!(in_dir("/m/memory", path), want, "{path}");
        }
        assert!(!in_dir("", "/m/memory/a.md"));
    }
}
