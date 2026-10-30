//! @name at the start of a message sends the rest to that agent: from the
//! Prompt instead of starting a session, from a Session instead of its
//! own. Later in the message, the Prompt starts a new session and a
//! Session sends to its own agent; either is told how to message the one
//! tagged. After Go's internal/ui/mention.go (mentionName :29, titleTag
//! :41, agentHandle :44, tags :50, nickname :63, mentionable :72,
//! mentioned :75, sendMentioned :93, atToken :111, completeMention :128,
//! mentionsIn :138, withMentions :165, mentionAgents :195, mentionMatches
//! :212, mentionRows :222), stream.go (handleTint :232) and
//! sessionviews.go (pickerRows :753). Sending to the agent tagged
//! (replyTo) is the session's: [`Act::ReplyTo`].

use rush_core::agent::Agent;
use rush_core::text::{Color, Line, Span};

use super::{Act, act, info};
use crate::model::Model;
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

/// The list's rows that are rooms (roomKeyPrefix).
pub const ROOM_KEY_PREFIX: &str = "room:";

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

/// Whether tag names agent a: its handle, its nickname or its title
/// (tags).
pub fn tags(a: &Agent, host_id: Option<&str>, tag: &str) -> bool {
    let eq = |s: &str| s.eq_ignore_ascii_case(tag);
    eq(&mention_name(a, host_id))
        || eq(&nickname(&a.key))
        || eq(&title_tag(&a.display_name))
        || eq(&one_line(&a.display_name)
            .split(' ')
            .collect::<Vec<_>>()
            .join("-"))
}

/// The list's agents in its order, each with its host's id when rush
/// runs it.
pub fn listed(m: &Model) -> Vec<(usize, Option<String>)> {
    m.order
        .iter()
        .map(|&i| (i, info(m, &m.agents[i].key).map(|h| h.id)))
        .collect()
}

/// The agent text starts by tagging, and the message after it
/// (mentioned).
pub fn mentioned(m: &Model, text: &str) -> Option<(usize, String)> {
    let tag = text.strip_prefix('@')?;
    let (tag, rest) = match tag.find(char::is_whitespace) {
        Some(i) => (&tag[..i], tag[i..].trim().to_string()),
        None => (tag, String::new()),
    };
    listed(m)
        .into_iter()
        .find(|(i, id)| mentionable(&m.agents[*i]) && tags(&m.agents[*i], id.as_deref(), tag))
        .map(|(i, _)| (i, rest))
}

/// Sends text to the agent it tags, if it tags one, from agent from's
/// session; tagged is the same text with its pastes marked
/// (sendMentioned). Pass when it tags none.
pub fn send_mentioned(m: &mut Model, from: &str, text: &str, tagged: &str) -> Out {
    let Some((i, rest)) = mentioned(m, text) else {
        return Out::Pass;
    };
    let a = m.agents[i].clone();
    if rest.is_empty() {
        m.flash_err(&format!(
            "type the message for {} after its tag",
            a.display_name
        ));
        return Out::Draw;
    }
    let tagged = match mentioned(m, tagged) {
        Some((_, t)) if !t.is_empty() => t,
        _ => tagged.to_string(),
    };
    act(
        m,
        from,
        Act::ReplyTo {
            to: a.key,
            text: rest,
            tagged,
        },
    )
}

use crate::frame::Out;

/// The @tag the cursor is in: where its @ and its end are, and what's
/// typed of it before the cursor. An @ inside a word (an email) is no
/// tag (atToken). back is how far from the end the cursor is.
pub fn at_token(input: &[char], back: usize) -> Option<(usize, usize, String)> {
    let cur = input.len().saturating_sub(back);
    let mut i = cur;
    while i > 0 && !input[i - 1].is_whitespace() && input[i - 1] != '@' {
        i -= 1;
    }
    if i == 0 || input[i - 1] != '@' || (i > 1 && !input[i - 2].is_whitespace()) {
        return None;
    }
    let mut end = cur;
    while end < input.len() && !input[end].is_whitespace() {
        end += 1;
    }
    Some((i - 1, end, input[i..cur].iter().collect()))
}

/// input with the tag the cursor is in made @name, and the cursor after
/// it: back is how far from the end that is (completeMention).
pub fn complete_mention(input: &[char], back: usize, name: &str) -> (Vec<char>, usize) {
    let Some((at, end, _)) = at_token(input, back) else {
        return (input.to_vec(), back);
    };
    let tail: Vec<char> = input[end..]
        .iter()
        .copied()
        .skip_while(|c| c.is_whitespace())
        .collect();
    let mut out = input[..at].to_vec();
    out.extend(format!("@{name} ").chars());
    out.extend(&tail);
    (out, tail.len())
}

/// The agents text tags past its start, each once (mentionsIn).
pub fn mentions_in(m: &Model, text: &str) -> Vec<usize> {
    let list = listed(m);
    let mut out = Vec::new();
    for (i, f) in text.split_whitespace().enumerate() {
        let Some(tag) = f.strip_prefix('@') else {
            continue;
        };
        if i == 0 {
            continue;
        }
        let tag = tag.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"']);
        for (j, id) in &list {
            let a = &m.agents[*j];
            if mentionable(a) && tags(a, id.as_deref(), tag) && !out.contains(j) {
                out.push(*j);
            }
        }
    }
    out
}

/// How a session's shell runs this rush.
fn rush_exe() -> String {
    rush_sys::selfexe::resolved()
        .map_or_else(|| "rush".into(), |p| p.to_string_lossy().into_owned())
}

/// text for agent self_key's session ("" for a new one), with a note on
/// each agent it tags mid-message: who it is, and how its shell messages
/// it, in a system-reminder so the feed shows only what you typed
/// (withMentions).
pub fn with_mentions(m: &Model, text: &str, self_key: &str) -> String {
    let mut notes = Vec::new();
    for i in mentions_in(m, text) {
        let a = &m.agents[i];
        if a.key == self_key {
            continue;
        }
        let host = info(m, &a.key);
        let id = host.as_ref().map(|h| h.id.clone());
        let mut n = format!(
            "@{} is {}, another agent rush runs",
            mention_name(a, id.as_deref()),
            one_line(&a.display_name)
        );
        if !a.job.cwd.is_empty() {
            n.push_str(&format!(", in {}", a.job.cwd));
        }
        match &host {
            Some(h) => n.push_str(&format!(
                ". To message it, pipe the message to `{} session send {}` (add --now to reach it mid-turn).",
                rush_exe(),
                h.id
            )),
            None => n.push_str("; it isn't a rush session, so it can't be messaged from your shell."),
        }
        if !a.job.transcript_path.is_empty() {
            n.push_str(&format!(
                " Its conversation so far is in {}.",
                a.job.transcript_path
            ));
        }
        notes.push(n);
    }
    if notes.is_empty() {
        return text.to_string();
    }
    format!(
        "{text}\n\n<system-reminder>\n{}\n</system-reminder>",
        notes.join("\n")
    )
}

/// What the picker offers while a tag is typed, anywhere in the message:
/// the agents whose handle or name holds it, in the list's order
/// (mentionAgents).
pub fn mention_agents(m: &Model, input: &[char], back: usize) -> Vec<usize> {
    let Some((_, _, q)) = at_token(input, back) else {
        return Vec::new();
    };
    let q = q.to_lowercase();
    listed(m)
        .into_iter()
        .filter(|(i, id)| {
            let a = &m.agents[*i];
            mentionable(a)
                && format!("{} {}", mention_name(a, id.as_deref()), a.display_name)
                    .to_lowercase()
                    .contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The picker's picks: each completes to its handle (mentionMatches).
pub fn mention_matches(m: &Model, input: &[char], back: usize) -> Vec<String> {
    mention_agents(m, input, back)
        .into_iter()
        .map(|i| {
            let id = info(m, &m.agents[i].key).map(|h| h.id);
            mention_name(&m.agents[i], id.as_deref())
        })
        .collect()
}

/// What the mention picker says under its rows (mentionHow).
pub const MENTION_HOW: &str = "↑↓ · tab or enter tags · first, the message goes to them; later, the agent here is told how to reach them";

/// The picker: each agent by its name, with its section, branch and the
/// handle its tag puts in the message (mentionRows).
pub fn mention_rows(m: &Model, input: &[char], back: usize, sel: usize, w: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    for i in mention_agents(m, input, back) {
        let a = &m.agents[i];
        let mut d = Vec::new();
        if let Some(g) = m.group_of.get(&a.key).filter(|g| !g.is_empty()) {
            d.push(g.clone());
        }
        if !a.branch.is_empty() {
            d.push(a.branch.clone());
        }
        let id = info(m, &a.key).map(|h| h.id);
        let name = {
            let n = one_line(&a.display_name);
            if n.is_empty() {
                mention_name(a, id.as_deref())
            } else {
                n
            }
        };
        let handle = format!("@{}", mention_name(a, id.as_deref()));
        rows.push((name, d.join(" · "), handle));
    }
    picker_rows(&rows, sel, w, "", MENTION_HOW)
}

/// A picker's rows over the box: name, what it is, a tag at the right;
/// the one picked on the selection's ground with ▸ (pickerRows).
pub fn picker_rows(
    rows: &[(String, String, String)],
    sel: usize,
    w: usize,
    lead: &str,
    how: &str,
) -> Vec<Line> {
    let start = sel.saturating_sub(5);
    let end = rows.len().min(start + 6);
    let name_w = rows[start..end]
        .iter()
        .map(|r| r.0.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .min(28);
    let mut out = Vec::new();
    for (i, (name, desc, tag)) in rows.iter().enumerate().take(end).skip(start) {
        let t = if tag.is_empty() {
            String::new()
        } else {
            format!("  {tag}")
        };
        let room =
            10.max(w as isize - name_w as isize - 7 - rush_core::text::width(&t) as isize) as usize;
        let mut row = line([
            Span::new(fit_str(&format!("{lead}{name}"), name_w + 1), bold(BRIGHT)),
            plain("  "),
            sp(
                rush_draw::truncate(&rush_convo::render::key_word(&one_line(desc)), room, "…")
                    .into_owned(),
                DIM,
            ),
        ]);
        if !t.is_empty() {
            row.spans.push(sp(t, DIM));
        }
        if i == sel {
            let mut l = line([sp(" ▸ ", ORANGE)]);
            join(&mut l, row);
            out.push(on_ground(l, w, SEL_BG));
        } else {
            let mut l = line([plain("   ")]);
            join(&mut l, row);
            out.push(on_ground(l, w, CHROME_BG));
        }
    }
    let more = if rows.len() > end - start {
        format!("{} of {} · ", sel + 1, rows.len())
    } else {
        String::new()
    };
    out.push(on_ground(
        line([plain("   "), sp(format!("{more}{how}"), DIM)]),
        w,
        CHROME_BG,
    ));
    out
}
