//! An agent's handle, how it's tagged with @: the fixed animal name a
//! session rush runs chirps under, else the first words of its title,
//! else a nickname from its key. Tagging and sending to the one tagged
//! are screens/session_cards/mention.rs's. After Go's
//! internal/ui/mention.go (mentionName :29, titleTag :41, nickname :63,
//! mentionable :72) and stream.go (handleTint :232).

use rush_core::agent::Agent;
use rush_core::text::Color;

pub use crate::rooms::ROOM_KEY_PREFIX;
use crate::style::*;

const NICK_WORDS: &[&str] = &[
    "brave", "quick", "sleepy", "muddy", "lucky", "fuzzy", "jolly", "tiny", "grumpy", "sunny",
    "dizzy", "bold", "calm", "cheeky", "clever", "cosy", "daring", "eager", "fancy", "gentle",
    "happy", "humble", "keen", "lively", "loyal", "merry", "nimble", "plucky", "proud", "quiet",
    "rusty", "scruffy", "shy", "silly", "snappy", "speedy", "spry", "sturdy", "swift", "wily",
    "witty", "zesty",
];
const NICK_NAMES: &[&str] = &[
    "biscuit", "pickle", "waffles", "noodle", "pepper", "bean", "maple", "ziggy", "rocket",
    "bonnie", "mochi", "pudding", "scout", "banjo", "tofu", "nugget", "juniper", "dottie", "fudge",
    "pip", "heron", "robin", "wren", "finch", "puffin", "magpie", "kestrel", "plover", "osprey",
    "lark", "sparrow", "starling", "swift", "tern", "ibis", "egret", "pelican", "toucan", "kiwi",
    "dodo", "condor",
];

fn fnv32a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// key's nickname: the same key always gets the same one (nickname).
pub fn nickname(key: &str) -> String {
    let n = fnv32a(key);
    let w = NICK_WORDS.len() as u32;
    format!(
        "{}-{}",
        NICK_WORDS[(n % w) as usize],
        NICK_NAMES[((n / w) % NICK_NAMES.len() as u32) as usize]
    )
}

/// title's first three words that aren't filler, lowercase, dashed
/// (titleTag).
pub fn title_tag(title: &str) -> String {
    rush_room::community::tag(&one_line(title))
}

/// An agent's host id, when rush runs it: its handle is the animal name
/// that id gives it.
pub fn host_id(a: &Agent) -> Option<&str> {
    a.rush.then_some(a.job.id.as_str())
}

/// An agent's handle, how it's tagged: a session rush runs has the fixed
/// animal name it chirps under on the feed, any other agent the first
/// few words of its title, with no title yet a nickname from its key
/// (mentionName). host_id is its host's id when rush runs it.
pub fn mention_name(a: &Agent, host_id: Option<&str>) -> String {
    if let Some(id) = host_id.filter(|id| !id.is_empty())
        && !a.key.starts_with(ROOM_KEY_PREFIX)
    {
        return rush_room::community::name(id);
    }
    let t = title_tag(&a.display_name);
    if !t.is_empty() {
        return t;
    }
    nickname(&a.key)
}

/// The colour a handle is painted (handleTint).
pub fn handle_tint(h: &str) -> Color {
    let palette = [BLUE, GREEN, YELLOW, QUEUE, ORANGE, RED];
    palette[(fnv32a(h) % palette.len() as u32) as usize]
}

/// Whether a message can reach it from here (mentionable).
pub fn mentionable(a: &Agent) -> bool {
    !a.interactive && !a.headless
}
