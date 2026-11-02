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
//! (replyTo) is the session's: [`Act::ReplyTo`]. The same picker offers
//! the session folder's files after its agents ([`mention_picks`],
//! mentionfiles.rs).

use rush_core::agent::Agent;
use rush_core::text::{Line, Span};

use super::{Act, act, info};
use crate::model::Model;
use crate::style::*;

// An agent's handle and its nickname are the frame's (crate::mention).
pub use rush_ui_base::mention::{
    ROOM_KEY_PREFIX, handle_tint, mention_name, mentionable, nickname, title_tag,
};

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
        .map(|&i| {
            // The agent's own id, connected or not (mentionName's a.ID).
            let id = super::handles::host_id(&m.agents[i]).map(str::to_string);
            (i, id)
        })
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
        let id = super::handles::host_id(a).map(str::to_string);
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
        .map(|i| mention_name(&m.agents[i], super::handles::host_id(&m.agents[i])))
        .collect()
}

/// What the mention picker says under its rows (mentionHow).
pub const MENTION_HOW: &str = "↑↓ · tab or enter tags · first, the message goes to them; later, the agent here is told how to reach them";

/// An agent as the picker shows it: its name, its section and branch,
/// and the handle its tag puts in the message.
fn agent_row(m: &Model, i: usize) -> (String, String, String) {
    let a = &m.agents[i];
    let mut d = Vec::new();
    if let Some(g) = m.group_of.get(&a.key).filter(|g| !g.is_empty()) {
        d.push(g.clone());
    }
    if !a.branch.is_empty() {
        d.push(a.branch.clone());
    }
    let id = super::handles::host_id(a);
    let name = {
        let n = one_line(&a.display_name);
        if n.is_empty() { mention_name(a, id) } else { n }
    };
    (name, d.join(" · "), format!("@{}", mention_name(a, id)))
}

/// The picker: each agent by its name, with its section, branch and the
/// handle its tag puts in the message (mentionRows).
pub fn mention_rows(m: &Model, input: &[char], back: usize, sel: usize, w: usize) -> Vec<Line> {
    let rows: Vec<_> = mention_agents(m, input, back)
        .into_iter()
        .map(|i| agent_row(m, i))
        .collect();
    picker_rows(&rows, sel, w, "", MENTION_HOW)
}

/// One of the @ picker's rows: an agent to message, or a file of the
/// session's folder, by its path from there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Agent(usize),
    File(String),
}

/// What the mention picker says under its rows with a file picked.
pub const FILE_HOW: &str = "↑↓ · tab or enter puts the file's path in the message";

/// What @ offers at the cursor, in a box whose session works in dir ("" for
/// none): the agents as [`mention_agents`] has them, then dir's files
/// that match, best first. Only files once what's typed has a / or a .,
/// as no handle does.
pub fn mention_picks(m: &mut Model, dir: &str, input: &[char], back: usize) -> Vec<Pick> {
    let Some((_, _, q)) = at_token(input, back) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if !q.contains(['/', '.']) {
        out.extend(mention_agents(m, input, back).into_iter().map(Pick::Agent));
    }
    let files = crate::mentionfiles::files_for(m, dir, &q);
    out.extend(files.iter().cloned().map(Pick::File));
    out
}

/// What a pick puts after the @: an agent's handle, or a file's path,
/// quoted when it has a space.
pub fn pick_token(m: &Model, p: &Pick) -> String {
    match p {
        Pick::Agent(i) => mention_name(&m.agents[*i], super::handles::host_id(&m.agents[*i])),
        Pick::File(f) => crate::mentionfiles::file_token(f),
    }
}

/// The folder session key works in: where its agent is now, else where
/// it began.
pub fn session_dir(m: &mut Model, key: &str) -> String {
    if let Some(a) = crate::sess::agent(m, key).filter(|a| !a.job.cwd.is_empty()) {
        return a.job.cwd.clone();
    }
    crate::sess::conn(m, key).info.began.cwd.clone()
}

/// The @ picker's rows, w wide, sel picked: agents as [`mention_rows`]
/// draws them, files by their path; with files among them, each kind
/// under a dim heading, "agents" then "files".
pub fn pick_rows(m: &Model, picks: &[Pick], sel: usize, w: usize) -> Vec<Line> {
    let headed = picks.iter().any(|p| matches!(p, Pick::File(_)));
    let start = sel.saturating_sub(5);
    let end = picks.len().min(start + 6);
    let agents: Vec<Option<(String, String, String)>> = picks[start..end]
        .iter()
        .map(|p| match p {
            Pick::Agent(i) => Some(agent_row(m, *i)),
            Pick::File(_) => None,
        })
        .collect();
    let name_w = agents
        .iter()
        .flatten()
        .map(|r| r.0.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .min(28);
    let mut out = Vec::new();
    for (i, p) in picks.iter().enumerate().take(end).skip(start) {
        let file = matches!(p, Pick::File(_));
        if headed && (i == start || file != matches!(picks[i - 1], Pick::File(_))) {
            let word = if file { "files" } else { "agents" };
            out.push(on_ground(line([plain(" "), sp(word, DIM)]), w, CHROME_BG));
        }
        let row = match (p, &agents[i - start]) {
            (Pick::File(f), _) => {
                let room = w.saturating_sub(6).max(10);
                let fw = rush_core::text::width(f);
                let shown = if fw > room {
                    crate::frame::sheet::truncate_left(f, fw - room + 1, "…")
                } else {
                    f.clone()
                };
                line([Span::new(shown, bold(BRIGHT))])
            }
            (_, Some((name, desc, tag))) => row_line(name, desc, tag, name_w, w, ""),
            _ => Line::new(),
        };
        out.push(framed(row, i == sel, w));
    }
    let how = match picks.get(sel) {
        Some(Pick::File(_)) => FILE_HOW,
        _ => MENTION_HOW,
    };
    out.push(foot(sel, picks.len(), end - start, how, w));
    out
}

/// A picker row's name, what it is and its tag at the right, w wide.
fn row_line(name: &str, desc: &str, tag: &str, name_w: usize, w: usize, lead: &str) -> Line {
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
    row
}

/// A picker row on its ground: the one picked on the selection's with ▸.
fn framed(row: Line, picked: bool, w: usize) -> Line {
    if picked {
        let mut l = line([sp(" ▸ ", ORANGE)]);
        join(&mut l, row);
        on_ground(l, w, SEL_BG)
    } else {
        let mut l = line([plain("   ")]);
        join(&mut l, row);
        on_ground(l, w, CHROME_BG)
    }
}

/// The line under a picker's rows: where the pick is when not all are
/// shown, and how.
fn foot(sel: usize, n: usize, shown: usize, how: &str, w: usize) -> Line {
    let more = if n > shown {
        format!("{} of {} · ", sel + 1, n)
    } else {
        String::new()
    };
    on_ground(
        line([plain("   "), sp(format!("{more}{how}"), DIM)]),
        w,
        CHROME_BG,
    )
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
        out.push(framed(
            row_line(name, desc, tag, name_w, w, lead),
            i == sel,
            w,
        ));
    }
    out.push(foot(sel, rows.len(), end - start, how, w));
    out
}
