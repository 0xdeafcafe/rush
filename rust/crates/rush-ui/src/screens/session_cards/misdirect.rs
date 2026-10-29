//! A message typed into the wrong session: before it goes, rush asks when
//! another live session is clearly what it's about. Each repo's newest
//! commit subjects are read off the UI thread every few minutes. After
//! Go's internal/ui/misdirect.go (misdirectState :19, refreshSubjects
//! :33, agentRoot :64, agentRepo :71, askMisdirected :84, takesMessageNow
//! :131, misdirected :140, matchScore :174, msgWords :206, subjectWords
//! :219, commonWords :232).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rush_core::agent::{Agent, State};
use rush_core::time::Ms;

use super::{Act, act, info, set_input};
use crate::frame::Out;
use crate::frame::confirm::{Confirm, Then};
use crate::model::Model;

/// What rush knows to tell a message typed into the wrong session
/// (misdirectState).
#[derive(Default)]
pub struct Misdirect {
    /// Each repo's newest commit subjects, newest first, by its root.
    pub subjects: HashMap<String, Vec<String>>,
    pub at: Ms,
    pub loading: bool,
    /// The send under way was told to go ahead regardless.
    pub skip: bool,
}

const SUBJECTS_EVERY: Ms = 2 * 60_000;

/// Reads the newest commit subjects of every repo a session works in,
/// every few minutes, off the UI thread (refreshSubjects). Only with a
/// loop: tests and --render never run git.
pub fn refresh_subjects(m: &mut Model) {
    if m.sender().is_none() {
        return;
    }
    let now = super::now(m);
    let st = m.kept::<Misdirect>();
    if st.loading || (st.at != 0 && now.saturating_sub(st.at) < SUBJECTS_EVERY) {
        return;
    }
    let roots: HashSet<String> = m
        .agents
        .iter()
        .map(agent_root)
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .collect();
    if roots.is_empty() {
        return;
    }
    m.kept::<Misdirect>().loading = true;
    m.later(
        move || {
            let mut out = HashMap::new();
            for r in roots {
                let got = rush_sys::cmd::output_within(
                    std::process::Command::new("git").args(["-C", &r, "log", "-5", "--format=%s"]),
                    std::time::Duration::from_secs(2),
                );
                if let Ok(o) = got
                    && o.ok()
                {
                    let b = String::from_utf8_lossy(&o.out);
                    out.insert(r, b.trim().split('\n').map(str::to_string).collect());
                }
            }
            out
        },
        move |m, subjects| {
            let st = m.kept::<Misdirect>();
            st.subjects = subjects;
            st.at = now;
            st.loading = false;
            Out::Idle
        },
    );
}

/// Where an agent works: its repo's root, else its folder (agentRoot).
pub fn agent_root(a: &Agent) -> &str {
    &a.job.cwd
}

/// Its repo's name (agentRepo).
pub fn agent_repo(a: &Agent) -> String {
    let base = |p: &str| {
        Path::new(p)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if !a.repo.is_empty() {
        return base(&a.repo);
    }
    let r = agent_root(a);
    if r.is_empty() { String::new() } else { base(r) }
}

/// Asks before a message typed into agent key's session goes there when
/// another live session is clearly what it's about: enter (or space)
/// forwards it, s sends it here anyway with send, esc leaves it in the
/// box to edit. False when nothing stands out (askMisdirected).
pub fn ask_misdirected(m: &mut Model, key: &str, text: &str, send: Then) -> bool {
    if m.kept::<Misdirect>().skip || text.contains("[Image #") {
        return false;
    }
    let Some(here) = m.agents.iter().position(|a| a.key == key) else {
        return false;
    };
    let others: Vec<usize> = (0..m.agents.len())
        .filter(|&i| i != here && takes_message_now(m, &m.agents[i]))
        .collect();
    let subjects = std::mem::take(&mut m.kept::<Misdirect>().subjects);
    let found = misdirected(text, &m.agents[here], &others, &m.agents, &subjects);
    m.kept::<Misdirect>().subjects = subjects;
    let Some((to, words)) = found else {
        return false;
    };
    let to = m.agents[to].clone();
    let (k, t) = (key.to_string(), text.to_string());
    let mut c = Confirm::new(
        format!("Looks meant for {} ({})", to.display_name, agent_repo(&to)),
        move |m: &mut Model| {
            set_input(m, &k, "");
            act(
                m,
                &k,
                Act::ReplyTo {
                    to: to.key.clone(),
                    text: t.clone(),
                    tagged: t,
                },
            )
        },
    )
    .detail(format!(
        "it mentions {} · esc leaves it here to edit",
        words.join(", ")
    ));
    c.yes_text = "send there".into();
    // Space says yes too, beside enter.
    c.again = "space".into();
    c.more.push((
        "s".into(),
        "send here anyway".into(),
        Box::new(move |m: &mut Model| {
            m.kept::<Misdirect>().skip = true;
            let out = send(m);
            m.kept::<Misdirect>().skip = false;
            out
        }),
    ));
    m.ask(c);
    true
}

/// Whether a forwarded message would reach a: a session rush hosts, or
/// one of another agent's at work now (takesMessageNow).
pub fn takes_message_now(m: &Model, a: &Agent) -> bool {
    if a.past || a.interactive || a.headless || matches!(a.job.state, State::Done | State::Stopped)
    {
        return false;
    }
    info(m, &a.key).is_some() || a.job.live() || a.pid != 0
}

/// The session among others that text is clearly about rather than
/// here, and the words that say so; None when nothing stands out
/// (misdirected).
pub fn misdirected(
    text: &str,
    here: &Agent,
    others: &[usize],
    agents: &[Agent],
    subjects: &HashMap<String, Vec<String>>,
) -> Option<(usize, Vec<String>)> {
    let words = msg_words(text);
    if words.is_empty() {
        return None;
    }
    let (here_score, _) = match_score(&words, here, subjects);
    let mut best = None;
    let mut best_words = Vec::new();
    let (mut top, mut second) = (0, 0);
    for &i in others {
        let (s, hit) = match_score(&words, &agents[i], subjects);
        if s > top {
            best = Some(i);
            best_words = hit;
            second = top;
            top = s;
        } else if s > second {
            second = s;
        }
    }
    // Clearly better: enough on its own, well above here, and alone at
    // the top.
    let best = best?;
    if top < 3 || top < 2 * here_score + 3 || top == second {
        return None;
    }
    // Another repo, and more than one word of its own or its name.
    let (there, mine) = (agent_repo(&agents[best]), agent_repo(here));
    if there.is_empty()
        || there == mine
        || (best_words.len() < 2 && !best_words.contains(&there.to_lowercase()))
    {
        return None;
    }
    Some((best, best_words))
}

/// Weighs the words of a message a session's context also has: its repo
/// by name most, its newest commit and its name next, the rest one
/// (matchScore).
pub fn match_score(
    words: &[String],
    a: &Agent,
    subjects: &HashMap<String, Vec<String>>,
) -> (i32, Vec<String>) {
    let mut bag: HashMap<String, i32> = HashMap::new();
    let mut add = |s: &str, w: i32| {
        for t in subject_words(s) {
            let e = bag.entry(t).or_insert(0);
            *e = (*e).max(w);
        }
    };
    add(&a.branch, 2);
    add(&a.job.name, 2);
    add(&a.display_name, 2);
    add(&a.job.intent, 2);
    add(&a.job.detail, 1);
    for t in &a.job.todos {
        add(&t.label, 1);
    }
    if let Some(ss) = subjects.get(agent_root(a)) {
        for (i, s) in ss.iter().enumerate() {
            add(s, 0.max(3 - i as i32)); // the newest commit counts most
        }
    }
    let r = agent_repo(a).to_lowercase();
    if r.len() >= 3 {
        bag.insert(r, 6);
    }
    let mut score = 0;
    let mut hit = Vec::new();
    for w in words {
        if let Some(&v) = bag.get(w)
            && v > 0
        {
            score += v;
            hit.push(w.clone());
        }
    }
    (score, hit)
}

/// A message's distinct words worth matching on (msgWords).
pub fn msg_words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    subject_words(text)
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// The lowercased words of s, three letters or more, without the words
/// every session shares (subjectWords).
pub fn subject_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit())
        .filter(|f| f.len() >= 3 && !COMMON_WORDS.contains(f))
        .map(str::to_string)
        .collect()
}

// A fixed list of generic words (commonWords).
const COMMON_WORDS: &[&str] = &[
    "the", "and", "for", "you", "all", "but", "not", "are", "was", "can", "its", "it's", "this",
    "that", "with", "from", "have", "has", "had", "need", "needs", "make", "should", "would",
    "could", "will", "into", "what", "when", "then", "them", "there", "their", "they", "just",
    "also", "more", "some", "like", "please", "use", "using", "any", "one", "two", "now", "new",
    "get", "got", "let", "see", "run", "ran", "out", "off", "our", "your", "yes", "why", "how",
    "who", "did", "does", "done", "keep", "going", "continue", "test", "tests", "lint", "build",
    "commit", "push", "merge", "branch", "review", "check", "error", "errors", "change", "changes",
    "update", "code", "file", "files", "work", "feat", "fix", "chore", "refactor", "docs", "main",
    "stuff", "thing", "things", "good", "fine", "okay", "same", "here", "thanks", "over", "still",
    "back", "wip", "ship", "add", "remove", "try", "again", "first", "last", "next", "sure",
];
