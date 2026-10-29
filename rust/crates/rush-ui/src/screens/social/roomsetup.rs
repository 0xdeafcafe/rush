//! A room's set-up: choosing its topic and panel (#room new, #room
//! <topic>), or a room on a chat (#discuss, #intervene: the chat's latest
//! exchanges go in as context, as many as you choose). The room itself is
//! a row in the list, shown as any agent is (rooms.rs). After Go's
//! internal/ui/room.go (roomSetup :18, openRoom :40, roomSetup :56,
//! showRoom :72, width :79, body :81, paste :89, choices :98, roomChoice
//! :112, key :122, create :177, draw :227) and discuss.go (chatRoom :108,
//! brief :162, joinedOpened :187).
//!
//! The members' configs (configAs, UseAgent) are made and the room kept
//! and started (room.Create, room.Start) through the hosts, off the UI
//! thread.

use rush_core::adapter::Registry;
use rush_core::text::Line;
use rush_room::room::{self as r, DEFAULT_ROUNDS, Member, Room};

use super::rooms::{ROOM_KEY_PREFIX, RoomRows};
use crate::frame::editor::{clean_paste, edit, insert};
use crate::frame::sheet::{sheet_row, text_field, window};
use crate::frame::{Out, Press, Sheet};
use crate::model::Model;
use crate::screens::accounts::setup::{
    StartOver, billing_of, models, start_defaults, start_routes,
};
use crate::screens::session_cards::discuss::{RoomAsk, brief, ctx_words};
use crate::screens::session_core::host::hosts;
use crate::style::*;

/// The panel chosen last, to start the next set-up with (lastPanel).
#[derive(Default)]
struct LastPanel(Vec<StartOver>);

/// Which set-up a room opened off the UI thread was made from.
#[derive(Default)]
struct Serial(u64);

/// The set-up whose room opened last: it closes, whether it's on screen
/// or (with no loop) still has the key.
#[derive(Default)]
struct Opened(Option<u64>);

/// The most agents on a panel, and the most rounds before a verdict.
const MOST_AGENTS: usize = 6;
const MOST_ROUNDS: i64 = 12;

/// #room new: a room's topic and panel being chosen (roomSetup).
#[derive(Default)]
pub struct RoomSetup {
    pub topic: Vec<char>,
    pub pos: usize,
    pub options: Vec<StartOver>,
    pub selected: Vec<StartOver>,
    pub cursor: usize,
    pub on_list: bool,
    pub rounds: i64,
    pub dir: String,
    /// A room on a chat (#discuss): the chat's agent key, its exchanges as
    /// the members would read them, oldest first, and how many of the
    /// latest go in. fill puts the verdict in the chat's box.
    pub from: String,
    pub turns: Vec<String>,
    pub ctx: usize,
    pub on_ctx: bool,
    pub fill: bool,
    /// Where the chat stands, ahead of its turns (#intervene).
    pub head: String,
    pub(super) serial: u64,
}

/// #room: the newest room's row, else a new room; #room new or #room
/// <topic> sets one up (openRoom).
pub fn open_room_cmd(m: &mut Model, arg: &str) -> Out {
    let arg = arg.trim();
    if !arg.is_empty() {
        return room_setup(m, if arg == "new" { "" } else { arg });
    }
    m.later(
        || r::Rooms::new().list().into_iter().next(),
        |m, newest| match newest {
            Some(r) => show_room(m, r),
            None => room_setup(m, ""),
        },
    );
    Out::Draw
}

/// Opens a room's set-up on topic: each agent that runs here at its
/// default, the panel chosen last picked again, its models read off the
/// UI thread (roomSetup).
pub fn room_setup(m: &mut Model, topic: &str) -> Out {
    let topic: Vec<char> = topic.chars().collect();
    let serial = {
        let s = m.kept::<Serial>();
        s.0 += 1;
        s.0
    };
    let mut s = RoomSetup {
        pos: topic.len(),
        topic,
        rounds: DEFAULT_ROUNDS,
        dir: crate::screens::list_extras::prompt::start_dir(m),
        selected: m.kept::<LastPanel>().0.clone(),
        serial,
        ..RoomSetup::default()
    };
    let mut kinds = Vec::new();
    for route in start_routes(m) {
        for kind in &route.kinds {
            let mut o = start_defaults(m, kind);
            o.billing = billing_of(&m.reg, &route.id, kind);
            s.options.push(o);
            kinds.push(kind.clone());
        }
    }
    m.open_sheet(Box::new(s));
    for k in kinds {
        crate::screens::accounts::startsheet::load_models(m, &k);
    }
    Out::Draw
}

/// A room's set-up on a chat, its latest turns as context to start
/// (chatRoom): the cards' Act::Room.
pub fn chat_room(m: &mut Model, ask: RoomAsk) -> Out {
    let out = room_setup(m, &ask.topic);
    if let Some(s) = m.sheet_as::<RoomSetup>() {
        s.ctx = ask.ctx.min(ask.turns.len());
        s.from = ask.from;
        s.turns = ask.turns;
        s.fill = ask.fill;
        s.head = ask.head;
        if !ask.dir.is_empty() {
            s.dir = ask.dir;
        }
    }
    out
}

/// Picks room r's row and opens it (showRoom).
fn show_room(m: &mut Model, room: Room) -> Out {
    let key = format!("{ROOM_KEY_PREFIX}{}", room.id);
    // Its row from now on, until the rooms are read again: its driver
    // has just been started.
    let rr = m.kept::<RoomRows>();
    if !rr.list.iter().any(|s| s.room.id == room.id) {
        let mut s = r::summarise(&room, &[]);
        s.running = true;
        rr.list.insert(0, s);
        m.rebuild();
    }
    m.select(key.clone());
    super::rooms::open_room(m, &key);
    m.load_rooms();
    Out::Draw
}

/// Each agent here at its default, and at each of its models (choices).
pub fn choices(m: &Model, options: &[StartOver]) -> Vec<StartOver> {
    let mut out = Vec::new();
    for o in options {
        out.push(o.clone());
        for c in models(m, &o.kind) {
            if c.id != o.model {
                let mut with = o.clone();
                with.model = c.id;
                out.push(with);
            }
        }
    }
    out
}

/// One choice as its row says it: its model, harness and provider, and
/// the model's id (roomChoice).
pub fn room_choice(reg: &Registry, o: &StartOver) -> Line {
    let word = crate::screens::accounts::look::model_word(reg, &o.kind, &o.model);
    let mut s = if word.is_empty() {
        "default model".to_string()
    } else {
        word
    };
    s.push_str(&format!(
        " · {} · {}",
        reg.harness_label(&o.kind),
        reg.provider_label(&reg.provider_of(&o.kind))
    ));
    if o.billing == rush_state::profiles::BILLING_KEY {
        s.push_str(" · API key");
    }
    line([plain(s), plain("  "), sp(o.model.clone(), FAINT)])
}

/// The agents rush knows, as the room names its members.
struct Named(Registry);

impl rush_room::Agents for Named {
    fn harness_label(&self, kind: &str) -> String {
        self.0.harness_label(kind)
    }
    fn model_name(&self, kind: &str, model: &str) -> String {
        self.0.model_name(kind, model)
    }
    fn supports_guide(&self, kind: &str) -> bool {
        self.0.supports(kind, rush_core::adapter::Feature::Guide)
    }
}

impl RoomSetup {
    /// The context chosen: the chat's latest ctx exchanges (brief).
    fn brief(&self) -> String {
        brief(&RoomAsk {
            turns: self.turns.clone(),
            ctx: self.ctx,
            head: self.head.clone(),
            ..RoomAsk::default()
        })
    }

    /// Opens the room set up: its panel's sessions start in its driver
    /// (create).
    fn create(&mut self, m: &mut Model) -> Out {
        let topic: String = self.topic.iter().collect();
        let topic = topic.trim().to_string();
        if topic.is_empty() {
            m.flash("give the room a topic first");
            return Out::Draw;
        }
        if self.selected.len() < 2 {
            m.flash("choose at least two agents: tab, then space");
            return Out::Draw;
        }
        let room = Room {
            topic,
            cwd: self.dir.clone(),
            rounds: self.rounds,
            from: self.from.clone(),
            brief: self.brief(),
            ..Room::default()
        };
        m.kept::<LastPanel>().0 = self.selected.clone();
        let starts: Vec<_> = self
            .selected
            .iter()
            .map(|o| {
                let context = models(m, &o.kind)
                    .into_iter()
                    .filter(|c| c.id == o.model)
                    .map(|c| c.context)
                    .next_back()
                    .unwrap_or(0);
                (
                    crate::screens::session_core::start::config_as(m, o, &self.dir),
                    context,
                )
            })
            .collect();
        let (h, named) = (hosts(m), Named(m.reg.clone()));
        let (serial, from, fill) = (self.serial, self.from.clone(), self.fill);
        m.later(
            move || -> Result<Result<Room, String>, String> {
                let mut room = room;
                let mut taken: Vec<String> = Vec::new();
                for (st, context) in starts {
                    // Its config's own words fail as they are (UseAgent).
                    let config = h.room_member(&st)?;
                    let mut mb = Member {
                        config,
                        context,
                        ..Member::default()
                    };
                    mb.name = r::name(&named, &mb.kind(), &mb.model(), &taken);
                    taken.push(mb.name.clone());
                    room.members.push(mb);
                }
                Ok(h.open_room(room))
            },
            move |m, res| match res {
                Err(e) => {
                    m.fail(&e);
                    Out::Draw
                }
                Ok(Err(e)) => {
                    m.fail(&format!("couldn't open the room: {e}"));
                    Out::Draw
                }
                Ok(Ok(room)) => {
                    m.kept::<Opened>().0 = Some(serial);
                    if m.sheet_as::<RoomSetup>()
                        .is_some_and(|s| s.serial == serial)
                    {
                        m.sheet = None;
                    }
                    if !from.is_empty() {
                        return joined_opened(m, fill);
                    }
                    show_room(m, room)
                }
            },
        );
        if m.kept::<Opened>().0 == Some(self.serial) {
            return Out::Close; // opened already: no loop
        }
        Out::Draw
    }

    /// The set-up's rows, h at most (draw).
    fn draw(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let on_topic = !self.on_list && !self.on_ctx;
        let mut topic = line([plain("  ")]);
        join(
            &mut topic,
            text_field(
                &self.topic,
                self.pos,
                on_topic,
                "What should they argue about?",
                w.saturating_sub(4),
            ),
        );
        let mut out = vec![
            line([spb("Topic", TEXT)]),
            topic,
            Line::new(),
            line([
                spb("Folder", TEXT),
                plain("  "),
                sp(tildify(&self.dir), SUB),
                sp(
                    "  where they read and run things: the selected agent's, as for a new session",
                    DIM,
                ),
            ]),
            Line::new(),
        ];
        if !self.from.is_empty() {
            let n = format!("← {} →", self.ctx);
            let n = if self.on_ctx {
                spb(n, ORANGE)
            } else {
                sp(n, BRIGHT)
            };
            out.push(line([
                spb("Context", TEXT),
                plain("  "),
                n,
                sp(format!("  {}", ctx_words(self.ctx, self.turns.len())), DIM),
            ]));
            out.push(Line::new());
        }
        out.push(line([
            spb("Panel", TEXT),
            sp(
                format!(
                    "  {} chosen · two or more, mixed models argue best",
                    self.selected.len()
                ),
                DIM,
            ),
            sp("   rounds at most: ", DIM),
            sp(self.rounds.to_string(), BRIGHT),
        ]));
        out.push(Line::new());
        let choices = choices(m, &self.options);
        if choices.is_empty() {
            out.push(line([sp(
                "No runnable agents. Add one in Settings first.",
                YELLOW,
            )]));
            return out;
        }
        self.cursor = self.cursor.min(choices.len() - 1);
        let (from, to) = window(
            choices.len(),
            self.cursor,
            1.max(h as isize - out.len() as isize) as usize,
        );
        for (i, c) in choices.iter().enumerate().take(to).skip(from) {
            let mut row = if self.selected.contains(c) {
                line([sp("● ", GREEN)])
            } else {
                line([plain("○ ")])
            };
            join(&mut row, room_choice(&m.reg, c));
            out.push(sheet_row(row, self.on_list && i == self.cursor, w));
        }
        out
    }
}

/// A room on a chat started: you stay in the chat, the room's row joins
/// the chat's in the list, and its verdict comes back (joinedOpened).
fn joined_opened(m: &mut Model, _fill: bool) -> Out {
    m.flash("room opened on this chat · it shows above the box · the verdict comes back here");
    m.load_rooms();
    Out::Draw
}

impl Sheet for RoomSetup {
    fn width(&self, _m: &Model) -> usize {
        110
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let hint = if self.from.is_empty() {
            "tab topic ↔ panel · space choose · ←→ rounds · enter open the room · esc back"
        } else {
            "tab topic → context → panel · ←→ turns of context, or rounds · space choose · enter open the room · esc back"
        };
        let mut out = self.draw(m, w, h.saturating_sub(2));
        out.push(Line::new());
        out.push(line([sp(hint, DIM)]));
        out
    }

    fn paste(&mut self, _m: &mut Model, text: &str) -> Out {
        if !self.on_list && !self.on_ctx {
            let r: Vec<char> = clean_paste(text).chars().collect();
            let at = self.pos.min(self.topic.len());
            self.topic = insert(&self.topic, at, &r);
            self.pos = at + r.len();
        }
        Out::Draw
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        let key = k.s.as_str();
        match key {
            "esc" => return Out::Close,
            "tab" | "shift+tab" => {
                if !self.from.is_empty() && !self.on_list && !self.on_ctx {
                    self.on_ctx = true; // topic, then context, then the panel
                } else if self.on_ctx {
                    self.on_ctx = false;
                    self.on_list = true;
                } else {
                    self.on_list = !self.on_list;
                }
                return Out::Draw;
            }
            "enter" => return self.create(m),
            _ => {}
        }
        if self.on_ctx {
            match key {
                "left" | "-" => self.ctx = self.ctx.saturating_sub(1),
                "right" | "+" | "=" => self.ctx = self.turns.len().min(self.ctx + 1),
                _ => return Out::Idle,
            }
            return Out::Draw;
        }
        if !self.on_list {
            return match edit(&self.topic, self.pos, k) {
                Some((t, p)) => {
                    self.topic = t;
                    self.pos = p;
                    Out::Draw
                }
                None => Out::Idle,
            };
        }
        let choices = choices(m, &self.options);
        match key {
            "up" => self.cursor = self.cursor.saturating_sub(1),
            "down" => self.cursor = choices.len().saturating_sub(1).min(self.cursor + 1),
            "left" | "-" => self.rounds = 2.max(self.rounds - 1),
            "right" | "+" | "=" => self.rounds = MOST_ROUNDS.min(self.rounds + 1),
            "space" | " " => {
                if let Some(o) = choices.get(self.cursor) {
                    if let Some(i) = self.selected.iter().position(|s| s == o) {
                        self.selected.remove(i);
                    } else if self.selected.len() < MOST_AGENTS {
                        self.selected.push(o.clone());
                    }
                }
            }
            _ => return Out::Idle,
        }
        Out::Draw
    }
}
