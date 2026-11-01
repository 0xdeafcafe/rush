//! Program status (OSC 7501): tells the terminal what rush's agents are
//! doing, for a tab, dock or sidebar that shows it: the whole fleet as the
//! root record, and an agent that's working, asking or stopped on an error
//! as one each beneath it. Terminals that don't know it ignore it. After
//! Go's internal/ui/progstatus.go (programStatus :33, statusState :76,
//! statusPrint :89, statusRecords :114, statusKind :161, statusID :171,
//! statusText :178). Not here yet: writing the records to the terminal,
//! which waits on rush-draw passing raw output through, so `Sent::step`
//! hands back the bytes to write; and the seen flag (rush's own state), so
//! every halt counts as unseen; and an untitled agent's @handle
//! (mention.go), its session id standing in.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};

use rush_core::agent::Agent;
use rush_fleet::Predicates;

use crate::style::one_line;

/// How many agents get a record of their own: terminals keep at least 64,
/// the root among them (statusMax).
pub const STATUS_MAX: usize = 63;

/// Removes every record rush made, for when it exits: an error record
/// would otherwise outlive it (ProgramStatusClear).
pub const CLEAR: &str = "\x1b]7501;state=clear\x1b\\";

/// What agent a's record reports: (state, kind, msg), state "" for none
/// (statusState).
pub fn state(a: &Agent, now: rush_core::time::Ms) -> (&'static str, &'static str, String) {
    if a.needs_you() {
        return ("blocked", kind(&a.job.needs), a.job.needs.clone());
    }
    if a.job.live() && !a.checking {
        return ("working", "", a.job.detail.clone());
    }
    if a.halted() {
        return ("error", "", a.halt_reason(now));
    }
    ("", "", String::new())
}

/// What an agent asking needs: leave to go on, or an answer (statusKind).
pub fn kind(needs: &str) -> &'static str {
    let n = needs.to_lowercase();
    if n.contains("permission") || n.contains("approv") || n.contains("allow") {
        "permission"
    } else {
        "question"
    }
}

/// An agent's title, else what stands in for its @handle.
fn title(a: &Agent) -> String {
    let t = one_line(&a.display_name);
    if t.is_empty() {
        format!("@{}", a.job.id)
    } else {
        t
    }
}

/// A hash of everything `records` reads (statusPrint).
pub fn print(agents: &[Agent], now: rush_core::time::Ms) -> u64 {
    let mut h = DefaultHasher::new();
    for a in agents {
        let (st, _, msg) = state(a, now);
        if st.is_empty() {
            continue;
        }
        (&a.key, st, &msg, &a.display_name).hash(&mut h);
        if a.display_name.trim().is_empty() {
            a.job.id.hash(&mut h);
        }
        1u8.hash(&mut h);
    }
    h.finish()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Each record's report, by id: "" is the root (statusRecords).
pub fn records(agents: &[Agent], now: rush_core::time::Ms) -> HashMap<String, String> {
    let mut recs = HashMap::new();
    let (mut blocked, mut working, mut failed) = (0, 0, 0);
    for a in agents {
        let (st, k, msg) = state(a, now);
        match st {
            "blocked" => blocked += 1,
            "working" => working += 1,
            "error" => failed += 1,
            _ => continue,
        }
        if recs.len() >= STATUS_MAX {
            continue; // counted at the root all the same
        }
        let id = id(&a.key);
        let mut r = format!("state={st}:id={id}");
        if !k.is_empty() {
            r += &format!(":kind={k}");
        }
        r += &format!(":title={}", text(&title(a), 192));
        let msg = one_line(&msg);
        if !msg.is_empty() {
            r += &format!(":msg={}", text(&msg, 2048));
        }
        recs.insert(id, r);
    }
    let root = if blocked > 0 {
        format!(
            "state=blocked:kind=question:msg={}",
            text(
                &format!("{blocked} agent{} waiting on you", plural(blocked)),
                2048
            )
        )
    } else if failed > 0 {
        format!(
            "state=error:msg={}",
            text(
                &format!("{failed} agent{} stopped on an error", plural(failed)),
                2048
            )
        )
    } else if working > 0 {
        format!(
            "state=working:msg={}",
            text(&format!("{working} agent{} working", plural(working)), 2048)
        )
    } else {
        "state=idle".to_string()
    };
    recs.insert(
        String::new(),
        format!("{root}:app=rush:title={}", text("rush", 192)),
    );
    recs
}

/// An agent's record id: its key, hashed to the letters ids may use
/// (statusID: the first six bytes of its SHA-1, in hex).
pub fn id(key: &str) -> String {
    sha1(key.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// s in base64, cut to n bytes first on a char boundary, its control
/// characters gone: a terminal refuses a report that carries one
/// (statusText).
pub fn text(s: &str, n: usize) -> String {
    let mut s: String = s
        .chars()
        .filter(|&c| !(c < '\x20' || c == '\x7f' || ('\u{80}'..'\u{a0}').contains(&c)))
        .collect();
    if s.len() > n {
        let mut i = n;
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        s.truncate(i);
    }
    base64(s.as_bytes())
}

/// The records last sent, and their print (Model's progStatus, progPrint).
#[derive(Default)]
pub struct Sent {
    recs: Option<HashMap<String, String>>,
    print: u64,
}

impl Sent {
    /// What to write so the terminal holds agents' records now: those
    /// that changed since last, and clears for those gone; "" when
    /// nothing changed (programStatus).
    pub fn step(&mut self, agents: &[Agent], now: rush_core::time::Ms) -> String {
        let p = print(agents, now);
        if self.recs.is_some() && p == self.print {
            return String::new();
        }
        self.print = p;
        let want = records(agents, now);
        let mut out = String::new();
        let had = self.recs.take().unwrap_or_default();
        let gone: BTreeMap<&String, ()> = had
            .keys()
            .filter(|id| !want.contains_key(*id))
            .map(|id| (id, ()))
            .collect();
        for id in gone.keys() {
            out += &format!("\x1b]7501;state=clear:id={id}\x1b\\");
        }
        // The root ("") first.
        let changed: BTreeMap<&String, &String> = want
            .iter()
            .filter(|(id, body)| had.get(*id) != Some(body))
            .collect();
        for body in changed.values() {
            out += &format!("\x1b]7501;{body}\x1b\\");
        }
        if out.is_empty() {
            self.recs = Some(had);
        } else {
            self.recs = Some(want);
        }
        out
    }
}

/// Standard base64, with padding.
pub fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// SHA-1 of b.
pub fn sha1(b: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = b.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((b.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut bb, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((bb & c) | (!bb & d), 0x5A827999),
                20..=39 => (bb ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((bb & c) | (bb & d) | (c & d), 0x8F1BBCDC),
                _ => (bb ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = bb.rotate_left(30);
            bb = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, bb, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}
