//! The sheets and panels drawn as Go draws them: each body below is Go's
//! own, printed by a throwaway test in internal/ui (ansi stripped, each
//! row's trailing blanks trimmed) from the same fixtures, at the widths
//! Go's sheets get. And the keys each answers, as tables.

use std::collections::HashMap;

use rush_core::adapter::FileRewind;
use rush_core::text::Line;
use rush_room::room::{Member, Room, Summary};

use super::asksheets::ExportSheet;
use super::btw::{Qa, Thread, thread_lines};
use super::compact::{CompactSheet, Summ};
use super::discuss::{Verdicts, chat_room_rows, verdict_rows};
use super::fork::{ForkSheet, ForkTurn};
use super::message::{MessageSheet, UserMessage};
use super::rewind::{Branch, FilesPreview, RewindSheet, TurnStart};
use super::sidekick::{Notes, Sidekick, side_column};
use super::tests::*;
use super::*;
use crate::frame::Sheet;

const GO_EXPORT: &str = r#"Export  the conversation as plain text, as Claude Code writes it

  3 lines · ~7 tok

  Copy to the clipboard
▍ Save to a file
    /src/app/conversation.txt · /export <name> saves elsewhere

↑↓ choose  ·  enter go  ·  esc cancel"#;
const GO_EXPORT_LOADING: &str = r#"Export  the conversation as plain text, as Claude Code writes it

  asking Claude Code for it…

esc close"#;
const GO_REWIND0: &str = r#"Rewind fix login  go back to before one of your messages and try again with less in its context

▍ turn 3 · the last   make it faster
  turn 2              add a test
  turn 1              fix the login bug

  or go back down a path you left
  ⑂ from turn 2       4 of your messages · left 3h ago · try redis

  It forgets turn 3. Your message goes back in the box to change and send again.
  The path you leave is kept: /rewind again to go back down it.

  c Code       keep it as it is now · only the conversation goes back
  n Bring back nothing · or a note of what was tried and found

↑↓ pick  ·  c code  ·  n note  ·  enter rewind  ·  esc cancel"#;
const GO_REWIND2: &str = r#"Rewind fix login  go back to before one of your messages and try again with less in its context

  turn 3 · the last   make it faster
  turn 2              add a test
▍ turn 1              fix the login bug

  or go back down a path you left
  ⑂ from turn 2       4 of your messages · left 3h ago · try redis

  It forgets turns 1–3 (3 of your messages). Your message goes back in the box.
  The path you leave is kept: /rewind again to go back down it.

  c Code       put back as they were before turn 1 · 2 files +12 −3
               /src/app/a.go, /src/app/b.go
  n Bring back a note of what was learned · Claude writes it first, one extra turn

↑↓ pick  ·  c code  ·  n note  ·  enter rewind  ·  esc cancel"#;
const GO_REWIND_BRANCH: &str = r#"Rewind fix login  go back to before one of your messages and try again with less in its context

  turn 3 · the last   make it faster
  turn 2              add a test
  turn 1              fix the login bug

  or go back down a path you left
▍ ⑂ from turn 2       4 of your messages · left 3h ago · try redis

  Carries on down that path from where it was left: 4 of your messages, apart from here since turn 2.
  This path is kept as a branch too. Files stay as they are now.

↑↓ pick  ·  enter switch  ·  esc cancel"#;
const GO_FORK0: &str = r#"Fork fix login  a copy of the conversation carries on as a new agent; this one stays as it is

▍ Name              fix login (fork)▏
  Remembers       ‹ the whole conversation · 3 turns ›
  Model           ‹ as now ›
  Effort          ‹ as now ›
  Permissions     ‹ as now ›
  Folder          ‹ same folder · /src/app (not a git repo, so no worktree) ›
  First message     optional: what to tell it first
                    ↳ added after what it remembers: read from the cache if used in the last hour

  [Fork]  enter

  It remembers everything so far, then goes its own way.

↑↓ move  ·  ←→ change  ·  enter fork  ·  esc cancel"#;
const GO_FORK1: &str = r#"Fork fix login  a copy of the conversation carries on as a new agent; this one stays as it is

  Name              fix login (fork)
▍ Remembers       ‹ up to turn 2 · add a test ›
  Model           ‹ sonnet ›
  Effort          ‹ as now ›
  Permissions     ‹ as now ›
  Folder          ‹ same folder · /src/app (not a git repo, so no worktree) ›
  First message     optional: what to tell it first
                    ↳ breaks the cache: another model reads all it remembers at full price

  [Fork]  enter

  It remembers turns 1–2 and forgets 3–3. This one keeps them all.

↑↓ move  ·  ←→ change  ·  enter fork  ·  esc cancel"#;
const GO_COMPACT: &str = r#"Compact  choose who writes the summary

▍ native harness compaction · default
  Claude Code       claude-haiku-4-5
  ! Ollama: not running

  External summaries start fresh; /rewind keeps the original.
  Native automatic compaction is unchanged.

↑↓ choose  ·  enter compact  ·  d make default  ·  esc cancel"#;
const GO_MESSAGE: &str = r#"Message 2 of 2  complete sent text and attachments
Text  ·  Image 1   tab

please look at this word word word word word word word word
word word word word word word word word word word word word
word word word word word word word word word word

↑↓ scroll · pgup/pgdown · [ ] messages · tab attachments · c copy · esc close"#;
const GO_VERDICT: &str = r#" ⇶ the room on this chat has a verdict · send it to this agent?
   asked: which cache to use

   alpha: use redis
   beta: use memcached for now

   enter send it to this agent  ·  e edit it first  ·  esc drop it"#;
const GO_CHATROOM: &str = r#"  ⇶ room · which cache                                    round 2/4 · 2 agents
      alpha  redis is simplest
      beta"#;
const GO_BTW: &str = r#"▍ btw  side question · not in the conversation
▍
▍ ❯ what is it doing?
▍   no answer came back: ask again, or ask in the
▍   conversation
▍ ❯ ▏ask…
▍ enter ask  ·  esc back to the chat
▍ ctrl+f its own chat  ·  ctrl+d close"#;
const GO_BTW_TUCKED: &str = r#"▏ btw  side question · not in the conversation
▏
▏ ask about what's going on: it doesn't go into t…
▏ ctrl+b ask more  ·  ctrl+d close"#;
const GO_RECALL: &str = r#" ◇ recall · sonnet · 0s

 IMPORTANT
 ✓ use redis
 ? who owns auth?

 RECAP
 ✓ keep the API
 ⚠ bypasses the store"#;

/// Rows as text, each row's trailing blanks trimmed, as the oracle.
fn rows(ls: &[Line]) -> String {
    ls.iter()
        .map(|l| l.plain().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end_matches('\n')
        .to_string()
}

fn body(s: &mut dyn Sheet, m: &Model, w: usize, h: usize) -> String {
    rows(&s.body(m, w, h))
}

fn rewind_sheet(m: &Model) -> RewindSheet {
    let ts = |n: u32, p: &str, u: &str| TurnStart {
        n,
        prompt: p.into(),
        uuid: u.into(),
        ..TurnStart::default()
    };
    RewindSheet {
        agent_key: "k".into(),
        agent: "fix login".into(),
        kind: "claude".into(),
        acct: Default::default(),
        sid: "s".into(),
        cwd: "/src/app".into(),
        path: String::new(),
        model: String::new(),
        turns: vec![ts(3, "make it faster", ""), ts(2, "add a test", ""), ts(1, "fix the login bug", "u1")],
        branches: vec![Branch {
            session_id: "b1".into(),
            left: now(m) - 3 * 3_600_000,
            from: 2,
            turns: 4,
            last: "try redis".into(),
        }],
        sel: 0,
        put_back: false,
        recap: false,
        previews: HashMap::new(),
    }
}

fn fork_sheet() -> ForkSheet {
    let ft = |n: u32, p: &str| ForkTurn { n, prompt: p.into(), end: 0 };
    let name: Vec<char> = "fix login (fork)".chars().collect();
    ForkSheet {
        conn: "k".into(),
        agent: "fix login".into(),
        prof: Default::default(),
        kind: "claude".into(),
        br: None,
        sid: "s".into(),
        cwd: "/src/app".into(),
        path: String::new(),
        turns: vec![ft(3, "make it faster"), ft(2, "add a test"), ft(1, "fix the login bug")],
        repo: String::new(),
        row: 0,
        name_pos: name.len(),
        name,
        first: Vec::new(),
        first_pos: 0,
        up_to: 0,
        model: 0,
        effort: 0,
        perm: 0,
        worktree: false,
        models: vec!["".into(), "opus".into(), "sonnet".into()],
        efforts: vec!["".into(), "high".into()],
        perms: vec!["".into(), "plan".into()],
        now_model: String::new(),
        now_effort: String::new(),
        now_perm: String::new(),
        cache_warm: 0,
        looked: true,
    }
}

/// /export, waiting and answered (asksheets.go:178 body).
#[test]
fn export_as_go_draws_it() {
    let m = hosted(120, 40);
    let mut s = ExportSheet {
        conn: "k".into(),
        text: "line one\nline two\nline three".into(),
        filename: "conversation.txt".into(),
        dir: "/src/app".into(),
        cur: 1,
        ..ExportSheet::default()
    };
    assert_eq!(body(&mut s, &m, 72, 30), GO_EXPORT);
    let mut s = ExportSheet { loading: true, ..ExportSheet::default() };
    assert_eq!(body(&mut s, &m, 72, 30), GO_EXPORT_LOADING);
}

/// /rewind on its last turn, on its first with the code put back, and on
/// a branch (rewindsheet.go:219 body).
#[test]
fn rewind_as_go_draws_it() {
    let m = hosted(120, 40);
    let mut s = rewind_sheet(&m);
    assert_eq!(body(&mut s, &m, 108, 34), GO_REWIND0);
    s.sel = 2;
    s.put_back = true;
    s.recap = true;
    s.previews.insert(
        "u1".into(),
        FilesPreview {
            loading: false,
            r: FileRewind {
                can_rewind: true,
                error: String::new(),
                files: vec!["/src/app/a.go".into(), "/src/app/b.go".into()],
                insertions: 12,
                deletions: 3,
            },
            err: String::new(),
        },
    );
    assert_eq!(body(&mut s, &m, 108, 34), GO_REWIND2);
    s.sel = 3;
    assert_eq!(body(&mut s, &m, 108, 34), GO_REWIND_BRANCH);
}

/// /fork as it opens, and remembering up to turn 2 on another model
/// (forksheet.go:223 body).
#[test]
fn fork_as_go_draws_it() {
    let m = hosted(120, 40);
    let mut s = fork_sheet();
    assert_eq!(body(&mut s, &m, 108, 34), GO_FORK0);
    s.row = super::fork::FROM;
    s.up_to = 1;
    s.model = 2;
    assert_eq!(body(&mut s, &m, 108, 34), GO_FORK1);
}

/// #compact with its own and one other summariser, and one that
/// couldn't say its models (compactsheet.go:116 body).
#[test]
fn compact_as_go_draws_it() {
    let m = hosted(120, 40);
    let mut s = CompactSheet {
        opts: vec![
            Summ { label: "native harness compaction".into(), ..Summ::default() },
            Summ {
                kind: "claude".into(),
                label: "Claude Code".into(),
                model: "claude-haiku-4-5".into(),
                prune: false,
            },
        ],
        errs: vec!["Ollama: not running".into()],
        ..CompactSheet::default()
    };
    assert_eq!(body(&mut s, &m, 72, 30), GO_COMPACT);
}

/// A message, its text cut at the sheet's width (messagesheet.go:148).
#[test]
fn message_as_go_draws_it() {
    let m = hosted(120, 40);
    let mut s = MessageSheet::new(
        vec![
            UserMessage { r#ref: "t1".into(), text: "first message".into(), images: vec![] },
            UserMessage {
                r#ref: "t2".into(),
                text: format!("please look at this {}", "word ".repeat(30)),
                images: vec!["/src/app/shot.png".into()],
            },
        ],
        1,
    );
    assert_eq!(body(&mut s, &m, 60, 20), GO_MESSAGE);
}

/// A room's verdict card, and the room running on the chat (discuss.go
/// verdictRows :247, chatRoomRows :226).
#[test]
fn rooms_as_go_draws_them() {
    let mut m = hosted(120, 40);
    let v = m.kept::<Verdicts>();
    v.waiting.insert(
        "k".into(),
        Summary {
            room: Room { topic: "which cache to use".into(), ..Room::default() },
            final_: "alpha: use redis\nbeta: use memcached for now".into(),
            ..Summary::default()
        },
    );
    let mut last = HashMap::new();
    last.insert("alpha".to_string(), "redis is simplest".to_string());
    let member = |n: &str| Member { name: n.into(), ..Member::default() };
    v.rooms = vec![Summary {
        room: Room {
            id: "r1".into(),
            from: "k".into(),
            topic: "which cache".into(),
            members: vec![member("alpha"), member("beta")],
            ..Room::default()
        },
        round: 2,
        last,
        ..Summary::default()
    }];
    assert_eq!(rows(&verdict_rows(&m, "k", 80)), GO_VERDICT);
    assert_eq!(rows(&chat_room_rows(&m, "k", 80)), GO_CHATROOM);
}

/// /btw's panel with the keys and a lost answer, and tucked away with
/// nothing asked (btwpanel.go:317 lines).
#[test]
fn btw_as_go_draws_it() {
    let mut t = Thread {
        focused: true,
        qa: vec![Qa { question: "what is it doing?".into(), response: String::new() }],
        err: "no answer came back: ask again, or ask in the conversation".into(),
        ..Thread::default()
    };
    assert_eq!(rows(&thread_lines(&mut t, 50, 20, 0)), GO_BTW);
    let mut t = Thread::default();
    assert_eq!(rows(&thread_lines(&mut t, 50, 20, 0)), GO_BTW_TUCKED);
}

/// Recall's column: what matters first, then the rest (sidekick.go:170).
#[test]
fn recall_as_go_draws_it() {
    let mut m = hosted(120, 40);
    let at = now(&m) - 500;
    m.kept::<Sidekick>().notes.insert(
        "k".into(),
        Notes {
            model: "sonnet".into(),
            at,
            decided: vec!["!use redis".into(), "keep the API".into()],
            grain: vec!["bypasses the store".into()],
            open: vec!["!who owns auth?".into()],
            ..Notes::default()
        },
    );
    assert_eq!(rows(&side_column(&mut m, "k", 40, 14, 0, 0)), GO_RECALL);
}
