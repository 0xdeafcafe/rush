//! #discuss and #intervene on a chat: a room's set-up with the chat's
//! latest exchanges as context, the room running on it in the dock, and
//! its verdict coming back as a card. After Go's internal/ui/discuss.go
//! (discuss :24, interveneBrief :35, intervene :40, chatStatus :87,
//! chatRoom :110, chatTurns :125, cutRunes :152, ctxWords :169,
//! discussTyped :193, chatRoomOn :214, chatRoomRows :226, verdictRows
//! :247, verdictText :266, verdictKey :272, verdictsBack :295). The
//! room's set-up sheet (roomSetup) and reading the rooms are the room
//! screens': [`Act::Room`] asks for one, [`verdicts_back`] takes the
//! rooms as they're read.

use std::collections::{HashMap, HashSet};

use rush_convo::{Item, Session, Status, Turn};
use rush_core::text::Line;
use rush_core::time::Ms;
use rush_room::room::{DEFAULT_ROUNDS, Summary};

use super::util::cut_runes;
use super::{Act, act, conn, info, open_key};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// A room's set-up on a chat, for the room screens to open: its topic,
/// the chat's exchanges, how many of the latest go in as context, where
/// it runs, a status to lead the context, and whether the verdict fills
/// the box when it comes (roomSetup's fields chatRoom sets).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomAsk {
    pub topic: String,
    pub from: String,
    pub turns: Vec<String>,
    pub ctx: usize,
    pub dir: String,
    pub head: String,
    pub fill: bool,
}

/// What #intervene's panel is asked about the chat (interveneBrief).
pub const INTERVENE_BRIEF: &str = "Go through this chat: its plan, its status and its latest turns. Is the agent making progress, and if not, why not (looping, the wrong approach, fixing a symptom, waiting on something)? Cite what you saw: tool calls, errors, times. End with exactly what the agent must do next, written to the agent.";

/// #discuss and ctrl+] t on agent key's chat: the topic is what's given,
/// else what's typed in its box, else its latest message; with fill the
/// verdict goes into the box when it comes (discuss).
pub fn discuss(m: &mut Model, key: &str, topic: &str, fill: bool) -> Out {
    let mut topic = topic.trim().to_string();
    if topic.is_empty() {
        topic = info(m, key)
            .map(|i| i.input.trim().to_string())
            .unwrap_or_default();
    }
    if topic.is_empty()
        && let Some(t) = super::session(m, key).and_then(|s| s.turns.last())
    {
        topic = one_line(&t.prompt);
    }
    let turns = super::session(m, key)
        .map(|s| chat_turns(s, false))
        .unwrap_or_default();
    chat_room(m, key, topic, turns, 3, fill, String::new())
}

/// #intervene: a room that goes through the chat, says why it's stuck
/// or going wrong, and puts its instructions to the agent in the box
/// (intervene).
pub fn intervene(m: &mut Model, key: &str, focus: &str) -> Out {
    let mut topic = INTERVENE_BRIEF.to_string();
    let focus = focus.trim();
    if !focus.is_empty() {
        topic.push_str(&format!(" The user's concern: {focus}"));
    }
    let now = super::now(m);
    let (turns, head) = super::session(m, key)
        .map(|s| (chat_turns(s, true), chat_status(s, now)))
        .unwrap_or_default();
    chat_room(m, key, topic, turns, 10, true, head)
}

fn chat_room(
    m: &mut Model,
    key: &str,
    topic: String,
    turns: Vec<String>,
    ctx: usize,
    fill: bool,
    head: String,
) -> Out {
    let dir = info(m, key).map(|i| i.cwd).unwrap_or_default();
    let ctx = ctx.min(turns.len());
    act(
        m,
        key,
        Act::Room(RoomAsk {
            topic,
            from: key.into(),
            turns,
            ctx,
            dir,
            head,
            fill,
        }),
    )
}

/// Where a chat stands, for a panel going through it: live or idle and
/// since when, and its task list (chatStatus).
pub fn chat_status(s: &Session, now: Ms) -> String {
    let mut b = String::new();
    if let Some(t) = s.turns.last() {
        if t.live {
            b.push_str(&format!(
                "Status: working, turn {} running since {} ({} ago)\n",
                t.n,
                super::clock::hhmm(t.start),
                super::clock::round_min(now.saturating_sub(t.start))
            ));
        } else if t.end != 0 {
            b.push_str(&format!(
                "Status: idle since {} ({} ago)\n",
                super::clock::hhmm(t.end),
                super::clock::round_min(now.saturating_sub(t.end))
            ));
        }
    }
    if !s.tasks.is_empty() {
        b.push_str("Plan:\n");
        for t in &s.tasks {
            b.push_str(&format!("- [{}] {}\n", t.status, t.subject));
        }
    }
    b.trim().to_string()
}

/// A chat's exchanges as a member reads them, oldest first: what was
/// asked and what the agent said, each cut to a few KB; with steps, a
/// reviewer's: when each began, every tool call, what the failed ones
/// said and how the turn ended (chatTurns).
pub fn chat_turns(s: &Session, steps: bool) -> Vec<String> {
    chat_turns_of(&s.turns, s, steps)
}

/// chat_turns of some of s's turns.
pub fn chat_turns_of(turns: &[Turn], s: &Session, steps: bool) -> Vec<String> {
    let mut out = Vec::new();
    for t in turns {
        let mut b = String::new();
        if steps && t.start != 0 {
            b.push_str(&format!(
                "[turn {}, {}]\n",
                t.n,
                super::clock::date_time(t.start)
            ));
        }
        let p = t.prompt.trim();
        if !p.is_empty() {
            b.push_str(&format!("User: {p}\n"));
        }
        for it in &t.items {
            match it {
                Item::Text { text, .. } if !text.trim().is_empty() => {
                    b.push_str(&format!("Agent: {}\n", text.trim()));
                }
                Item::Step(id) if steps => {
                    let st = s.step(*id);
                    b.push_str(&format!(
                        "Tool: {} {}\n",
                        st.tool,
                        cut_runes(&one_line(st.input_json()), 200)
                    ));
                    if st.status == Status::Failed {
                        b.push_str(&format!(
                            "  failed: {}\n",
                            cut_runes(&one_line(&st.output), 300)
                        ));
                    }
                }
                _ => {}
            }
        }
        if steps && !t.err.is_empty() {
            b.push_str(&format!("Turn ended badly: {}\n", t.err));
        }
        if !b.is_empty() {
            out.push(cut_runes(&b, 4 << 10));
        }
    }
    out
}

/// The context chosen: the chat's latest ctx exchanges, the status
/// first when there is one (brief).
pub fn brief(r: &RoomAsk) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !r.head.is_empty() {
        parts.push(&r.head);
    }
    parts.extend(
        r.turns[r.turns.len().saturating_sub(r.ctx)..]
            .iter()
            .map(String::as_str),
    );
    parts.join("\n---\n")
}

/// What the context count means (ctxWords).
pub fn ctx_words(n: usize, total: usize) -> String {
    match (n, total) {
        (_, 0) => "the chat has nothing to give yet".into(),
        (0, _) => "the topic alone".into(),
        (1, _) => "the latest exchange of the chat".into(),
        _ => format!("the latest {n} of the chat's {total} exchanges"),
    }
}

/// Enter in the discuss send mode: a room's set-up on what's typed, the
/// box cleared (discussTyped). text is the box's, its pastes expanded.
pub fn discuss_typed(m: &mut Model, key: &str, text: &str) -> Out {
    super::set_input(m, key, "");
    discuss(m, key, text, true)
}

/// The rooms as last read, and the verdicts waiting on chats.
#[derive(Default)]
pub struct Verdicts {
    pub rooms: Vec<Summary>,
    /// The verdict waiting on each chat, by its agent key.
    pub waiting: HashMap<String, Summary>,
    picked: HashSet<String>,
}

/// Takes the rooms as they're read, and hands each finished room on a
/// chat its verdict back: a card on the chat (verdictsBack).
pub fn verdicts_back(m: &mut Model, list: Vec<Summary>) {
    let open = open_key(m);
    let mut flash = false;
    let v = m.kept::<Verdicts>();
    for s in &list {
        if s.room.from.is_empty() || !s.verdict || v.picked.contains(&s.room.id) {
            continue;
        }
        v.picked.insert(s.room.id.clone());
        v.waiting.insert(s.room.from.clone(), s.clone());
        if open.as_deref() == Some(s.room.from.as_str()) {
            flash = true;
        }
    }
    v.rooms = list;
    if flash {
        m.flash("the room's verdict is in · enter sends it on");
    }
}

/// The newest room opened from chat key still running (chatRoomOn).
pub fn chat_room_on<'a>(m: &'a Model, key: &str) -> Option<&'a Summary> {
    m.kept
        .peek::<Verdicts>()?
        .rooms
        .iter()
        .find(|s| s.room.from == key && s.over.is_empty() && !s.verdict)
}

/// The dock's rows for the room running on chat key: its topic and
/// round, then each member and the first line of what it said last, the
/// one speaking spun (chatRoomRows).
pub fn chat_room_rows(m: &Model, key: &str, w: usize) -> Vec<Line> {
    let Some(r) = chat_room_on(m, key) else {
        return Vec::new();
    };
    let rounds = if r.room.rounds == 0 {
        DEFAULT_ROUNDS
    } else {
        r.room.rounds
    };
    let mut right = line([sp(
        format!(
            "round {}/{rounds} · {} agents",
            r.round,
            r.room.members.len()
        ),
        SUB,
    )]);
    right.spans.push(plain("  "));
    if r.paused {
        let mut p = line([sp("paused", YELLOW), sp(" · ", DIM)]);
        join(&mut p, right);
        right = p;
    }
    let room = 10.max(w as isize - right.width() as isize - 14) as usize;
    let mut left = line([plain("  "), sp("⇶ room", BLUE), sp(" · ", DIM)]);
    left.spans.push(spb(
        rush_draw::truncate(&one_line(&r.room.topic), room, "…").into_owned(),
        TEXT,
    ));
    let gap = w.saturating_sub(left.width() + right.width());
    left.spans.push(plain(blanks(gap)));
    join(&mut left, right);
    let mut out = vec![left];
    for (i, mb) in r.room.members.iter().enumerate() {
        let speaking = r.speaking.contains(&mb.name);
        let (mark, name) = if speaking {
            (
                sp(SPINNER[(m.tick + i) % SPINNER.len()], ORANGE),
                spb(&mb.name, ORANGE),
            )
        } else {
            (plain(" "), sp(&mb.name, TEXT))
        };
        let last = r
            .last
            .get(&mb.name)
            .map(|s| one_line(s))
            .unwrap_or_default();
        out.push(fit(
            line([
                plain("    "),
                mark,
                plain(" "),
                name,
                plain("  "),
                sp(last, DIM),
            ]),
            w.saturating_sub(2),
        ));
    }
    out
}

/// The dock's card for the verdict waiting on chat key (verdictRows).
pub fn verdict_rows(m: &Model, key: &str, w: usize) -> Vec<Line> {
    let Some(v) = m.kept.peek::<Verdicts>().and_then(|v| v.waiting.get(key)) else {
        return Vec::new();
    };
    let mut out = vec![
        line([
            plain(" "),
            spb("⇶ the room on this chat has a verdict", BLUE),
            sp(" · send it to this agent?", DIM),
        ]),
        line([
            plain("   "),
            sp(
                rush_draw::truncate(
                    &format!("asked: {}", one_line(&v.room.topic)),
                    10.max(w.saturating_sub(6)),
                    "…",
                )
                .into_owned(),
                DIM,
            ),
        ]),
        Line::new(),
    ];
    for l in v.final_.trim().split('\n') {
        for r in wrap(l, 20.max(w.saturating_sub(6))) {
            out.push(line([plain("   "), sp(r, TEXT)]));
        }
    }
    out.push(Line::new());
    let mut k = line([plain("   ")]);
    join(
        &mut k,
        keys(&[
            ("enter", "send it to this agent"),
            ("e", "edit it first"),
            ("esc", "drop it"),
        ]),
    );
    out.push(k);
    out
}

/// A verdict as it goes to the agent (verdictText).
pub fn verdict_text(s: &Summary) -> String {
    format!(
        "The room's verdict on {}:\n{}",
        one_line(&s.room.topic),
        s.final_.trim()
    )
}

/// The verdict card's keys, while the box is empty and no other card is
/// up: enter sends it on, e puts it in the box to edit, esc drops it
/// (verdictKey).
pub fn verdict_key_hook(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    if !matches!(s, "enter" | "e" | "esc") {
        return Out::Pass;
    }
    let Some(key) = open_key(m) else {
        return Out::Pass;
    };
    let Some(inf) = info(m, &key) else {
        return Out::Pass;
    };
    // Behind any other card; on it, or with nothing else picked.
    let has = m.kept::<Verdicts>().waiting.contains_key(&key);
    let sel = super::session_view(m, &key)
        .map(|v| v.sel.clone())
        .unwrap_or_default();
    if !has
        || !inf.input.is_empty()
        || !super::pending(m, &key).is_empty()
        || inf.decisions.is_some()
        || (!sel.is_empty() && !conn(m, &key).card_focus)
    {
        return Out::Pass;
    }
    let v = m.kept::<Verdicts>().waiting.remove(&key).expect("there");
    conn(m, &key).card_focus = false;
    if s == "esc" {
        m.flash("verdict dropped");
        return Out::Draw;
    }
    let text = verdict_text(&v);
    if s == "e" {
        super::set_input(m, &key, &text);
        return Out::Draw;
    }
    act(m, &key, Act::Send(text))
}
