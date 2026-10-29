//! The driver against fake sessions: each answers a prompt at once with
//! its next scripted turn, as a session's events would come.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use rush_core::event::{Approval, Event, Message, Part, PartKind, TurnEnd};
use rush_core::tool::{Call, Kind};
use rush_state::json::RawJson;

use super::*;
use crate::room::{Member, Room, Rooms};
use crate::tests::NoAgents;

/// A scripted turn: what the member says, or something else it does.
#[derive(Clone)]
enum Act {
    Say(&'static str),
    /// It asks to edit a file, then says this.
    EditThenSay(&'static str),
    /// You say this while it works; then it says the text, if any.
    User(Box<Entry>, &'static str),
}

#[derive(Default)]
struct Fake {
    scripts: Mutex<HashMap<usize, VecDeque<Act>>>,
    spawned: Mutex<Vec<Value>>,
    prompts: Arc<Mutex<Vec<(usize, String)>>>,
    denied: Arc<Mutex<Vec<String>>>,
}

struct FakeSession {
    i: usize,
    tx: SyncSender<Msg>,
    script: VecDeque<Act>,
    prompts: Arc<Mutex<Vec<(usize, String)>>>,
    denied: Arc<Mutex<Vec<String>>>,
    queue: Mutex<VecDeque<Act>>,
}

fn said(text: &str) -> Vec<MemberEvent> {
    vec![
        MemberEvent::Event(Box::new(Event::Message(Message {
            role: "assistant".into(),
            parts: vec![Part {
                kind: PartKind::Text,
                text: text.into(),
                ..Part::default()
            }],
            ..Message::default()
        }))),
        MemberEvent::Event(Box::new(Event::TurnEnd(TurnEnd {
            reason: "done".into(),
            ..TurnEnd::default()
        }))),
        MemberEvent::Info(SessionInfo {
            kind: "codex".into(),
            state: "idle".into(),
            ..SessionInfo::default()
        }),
    ]
}

impl Session for FakeSession {
    fn send(&self, text: &str) -> Result<(), String> {
        self.prompts
            .lock()
            .unwrap()
            .push((self.i, text.to_string()));
        let act = self.queue.lock().unwrap().pop_front();
        let msg = |evs| Msg::Member {
            i: self.i,
            evs,
            closed: false,
        };
        match act {
            Some(Act::Say(t)) => self.tx.send(msg(said(t))).unwrap(),
            Some(Act::EditThenSay(t)) => {
                let ask = MemberEvent::Event(Box::new(Event::Approval(Box::new(Approval {
                    id: "a1".into(),
                    call: Call {
                        id: "c1".into(),
                        name: "Edit".into(),
                        kind: Kind::Edit,
                        input: rush_core::tool::Input {
                            path: "/w/x.go".into(),
                            ..Default::default()
                        },
                        ..Call::default()
                    },
                    ..Approval::default()
                }))));
                self.tx.send(msg(vec![ask])).unwrap();
                self.tx.send(msg(said(t))).unwrap();
            }
            Some(Act::User(e, t)) => {
                self.tx.send(Msg::User(e)).unwrap();
                if !t.is_empty() {
                    self.tx.send(msg(said(t))).unwrap();
                }
            }
            None => self
                .tx
                .send(msg(said("(out of words) READY: yes")))
                .unwrap(),
        }
        Ok(())
    }
    fn send_guide(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn interrupt(&self) -> Result<(), String> {
        Ok(())
    }
    fn allow(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn deny(&self, id: &str, _: &str) -> Result<(), String> {
        self.denied.lock().unwrap().push(id.into());
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn close(&self) {
        let _ = &self.script;
    }
}

impl Sessions for Fake {
    fn spawn(&self, cfg: Value) -> Result<String, String> {
        let mut s = self.spawned.lock().unwrap();
        s.push(cfg);
        Ok(format!("s{}", s.len()))
    }
    fn dial(&self, _: &str, i: usize, tx: SyncSender<Msg>) -> Result<Box<dyn Session>, String> {
        let script = self.scripts.lock().unwrap().remove(&i).unwrap_or_default();
        Ok(Box::new(FakeSession {
            i,
            tx,
            script: VecDeque::new(),
            prompts: self.prompts.clone(),
            denied: self.denied.clone(),
            queue: Mutex::new(script),
        }))
    }
    fn log_event(&self, _: &str, _: &str, _: Map<String, Value>) {}
    fn log(&self, _: &str, _: Map<String, Value>) {}
    fn fail_report(&self, _: &str, err: &str, _: &str, _: i64) -> String {
        err.to_string()
    }
}

fn room(rooms: &Rooms, rounds: i64) -> Room {
    let cfg = |kind: &str| RawJson::from_value(&json!({"kind": kind, "prompt": "x"})).unwrap();
    rooms
        .create(
            Room {
                topic: "Tabs or spaces".into(),
                cwd: "/w".into(),
                rounds,
                members: vec![
                    Member {
                        name: "Opus".into(),
                        config: cfg("claude"),
                        context: 0,
                    },
                    Member {
                        name: "Codex".into(),
                        config: cfg("codex"),
                        context: 0,
                    },
                ],
                ..Room::default()
            },
            || "r1".into(),
        )
        .unwrap()
}

fn kinds(es: &[Entry]) -> Vec<String> {
    es.iter()
        .map(|e| format!("{}:{}", e.from, e.kind))
        .collect()
}

// Openings blind and together, then turns until everyone is READY: yes in
// the same round, then each one's final position as the verdict.
#[test]
fn a_room_converges_on_a_verdict() {
    let home = tempfile::tempdir().unwrap();
    let rooms = Rooms::at(home.path());
    room(&rooms, 4);
    let fake = Fake::default();
    fake.scripts.lock().unwrap().extend([
        (
            0,
            VecDeque::from([
                Act::Say("Tabs: they're one byte.\n\nREADY: no"),
                Act::Say("Codex is right about alignment. READY: yes"),
                Act::Say("Spaces,\n finally."),
            ]),
        ),
        (
            1,
            VecDeque::from([
                Act::Say("Spaces: they align. READY: no"),
                Act::Say("Agreed. **READY:** yes"),
                Act::Say("Spaces."),
            ]),
        ),
    ]);
    run(&rooms, "r1", &fake, &NoAgents).unwrap();
    let (es, _) = rooms.read("r1", 0).unwrap();
    assert_eq!(
        kinds(&es),
        [
            "room:info",
            "Opus:joined",
            "Codex:joined",
            "Opus:turn",
            "Codex:turn",
            "Opus:say",
            "Codex:say",
            "Opus:turn",
            "Opus:say",
            "Codex:turn",
            "Codex:say",
            "room:info",
            "room:verdict",
            "room:end",
        ]
    );
    assert_eq!(es[5].text, "Tabs: they're one byte.");
    assert_eq!((es[5].ready.as_str(), es[5].round), ("no", 1));
    assert_eq!(es[12].text, "Opus: Spaces, finally.\nCodex: Spaces.");
    assert_eq!(es[12].detail, "everyone said READY: yes in the same round");
    assert_eq!(es[13].text, "everyone said READY: yes in the same round");
    assert!(!rooms.pid_path("r1").exists());
    let spawned = fake.spawned.lock().unwrap();
    assert_eq!(spawned[0]["name"], "Room · Opus · Tabs or spaces");
    assert_eq!(
        spawned[0]["meta"],
        json!({"room": "r1", "roomMember": "Opus"})
    );
    assert_eq!(spawned[0]["cwd"], "/w");
    assert!(spawned[0].get("prompt").is_none());
    let prompts = fake.prompts.lock().unwrap();
    let (_, first) = &prompts[0];
    assert!(
        first.starts_with("You are Opus, one of 2 AI agents in a rush room"),
        "{first}"
    );
    assert!(
        first.contains("- Opus (claude)\n- Codex (codex)\n"),
        "{first}"
    );
    assert!(first.contains("after 4 rounds;"), "{first}");
    assert!(first.ends_with("give your own view, Opus. End with READY: yes or READY: no.\n"));
    // Blind openings: Codex's first prompt has nothing of Opus's.
    assert!(!prompts[1].1.contains("Tabs: they're"));
    let (_, second) = &prompts[2];
    assert!(
        second.starts_with(
            "\nNew in the room since your last turn:\n\nCodex:\nSpaces: they align.\nREADY: no\n"
        ),
        "{second}"
    );
    assert!(second.ends_with("It's your turn, Opus (round 2 of at most 4). Reply to the others by name, and end with READY: yes or READY: no.\n"));
    let (_, last) = &prompts[4];
    assert!(
        last.contains("The discussion is over: everyone said READY: yes in the same round."),
        "{last}"
    );
}

// /stop while a member speaks ends the room at once, with no verdict.
#[test]
fn stop_ends_the_room() {
    let home = tempfile::tempdir().unwrap();
    let rooms = Rooms::at(home.path());
    room(&rooms, 4);
    let fake = Fake::default();
    let stop = Entry {
        from: USER.into(),
        kind: CONTROL.into(),
        text: "stop".into(),
        ..Entry::default()
    };
    fake.scripts.lock().unwrap().extend([
        (
            0,
            VecDeque::from([Act::Say("Tabs. READY: no"), Act::User(Box::new(stop), "")]),
        ),
        (1, VecDeque::from([Act::Say("Spaces. READY: no")])),
    ]);
    run(&rooms, "r1", &fake, &NoAgents).unwrap();
    let (es, _) = rooms.read("r1", 0).unwrap();
    assert_eq!(es.last().unwrap().text, "stopped by you");
    assert!(!es.iter().any(|e| e.kind == VERDICT));
}

// A room changes no files: an edit is denied and said so; you speaking
// to one member puts its turn next.
#[test]
fn edits_are_denied_and_directed_words_go_first() {
    let home = tempfile::tempdir().unwrap();
    let rooms = Rooms::at(home.path());
    room(&rooms, 2);
    let fake = Fake::default();
    let to_codex = Entry {
        from: USER.into(),
        kind: SAY.into(),
        text: "@Codex what of Go?".into(),
        to: "Codex".into(),
        ..Entry::default()
    };
    fake.scripts.lock().unwrap().extend([
        (
            0,
            VecDeque::from([
                Act::EditThenSay("I'd change x.go. READY: yes"),
                Act::Say("Fine. READY: yes"),
                Act::Say("Tabs."),
            ]),
        ),
        (
            1,
            VecDeque::from([
                Act::User(Box::new(to_codex), "Go has a formatter. READY: no"),
                Act::Say("gofmt uses tabs. READY: yes"),
                Act::Say("Tabs."),
            ]),
        ),
    ]);
    run(&rooms, "r1", &fake, &NoAgents).unwrap();
    assert_eq!(*fake.denied.lock().unwrap(), ["a1"]);
    let (es, _) = rooms.read("r1", 0).unwrap();
    assert!(
        es.iter()
            .any(|e| e.kind == INFO && e.text == "Opus wasn't allowed to editing x.go"),
        "{:?}",
        kinds(&es)
    );
    let prompts = fake.prompts.lock().unwrap();
    // Codex's opening was cut by you speaking to it: its next prompt has
    // your words, to it.
    let codex: Vec<&String> = prompts
        .iter()
        .filter(|(i, _)| *i == 1)
        .map(|(_, p)| p)
        .collect();
    assert!(
        codex[1].contains("\nThe user (to Codex):\n@Codex what of Go?\n")
            && codex[1].contains("The user spoke to you directly: answer the user first.\n"),
        "{}",
        codex[1]
    );
}
