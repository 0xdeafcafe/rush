//! Settings › Usage (Rust-only; Go has no such page): the rules rush keeps
//! on its own to save tokens and money and to keep agents in check, in
//! one place. The prompt cache (rush_core::cachekeep and subkeep, the
//! accounts module's keep_warm, the host's subagent watch, the gate's
//! known-slow commands), the context guard (the bundled context-guard
//! plugin's rules, kept in its own settings, which its hook reads at every
//! call), the runaway guard (guard.rs) and the advisor. A config that says
//! nothing is today's rule.

use std::collections::BTreeMap;
use std::rc::Rc;

use rush_bundled::contextguard as cg;

use super::form::{Section, Setting, choice, set};
use super::guard::numbers;
use super::paint::on_off;
use super::{config, config_mut, dialog, dialog_mut};
use crate::frame::Out;
use crate::model::Model;

/// What the page shows of the context guard, read as it opens: whether
/// the plugin runs, and its settings' values.
#[derive(Clone, Debug, Default)]
pub struct GuardRead {
    pub on: bool,
    pub values: BTreeMap<String, String>,
}

/// Reads the context guard's settings as the page opens, off the UI: the
/// disk.
pub fn read(m: &mut Model) {
    m.later(
        || GuardRead {
            on: rush_plugin::bundled_on(cg::NAME),
            values: rush_plugin::setting_values_of(&cg::manifest()),
        },
        |m, r| {
            if let Some(d) = m.kept::<super::State>().dialog.as_mut() {
                d.guard = Some(r);
            }
            Out::Draw
        },
    );
}

/// The page's sections, in order.
pub fn sections(m: &Model) -> Vec<Section> {
    let mut s = cache_sections(m);
    s.push(context_guard_section(m));
    s.extend(super::cheap::sections(m));
    s.extend(super::guard::sections(m));
    s.push(advisor_section(m));
    s
}

/// Prompt cache: keeping idle conversations' caches warm (or compacting
/// them for a long wait), waiting subagents' caches, and the numbers both
/// go by.
pub fn cache_sections(m: &Model) -> Vec<Section> {
    let k = config(m).cache_keep.clone();
    let mut rows = vec![choice(
        "Keep idle conversations warm",
        on_off(k.on),
        "A provider keeps a session's prompt cached for a while after its last request (Claude: 5 minutes, or an hour), and a message after that pays to write the whole context again: 1.25× the input price for a 5-minute cache, 2× for an hour's. Just before an idle session's cache lapses, rush sends one small request that reads the whole cached context, a tenth of the price, and its life starts over. It isn't a message: nothing is added to the conversation, no turn runs, nothing notifies. It only does this for a session that went idle within the time below, with a context worth it; never one asleep, done, held, or on an account past its pace, nor while you're away. Each is in the session's events as cache-kept, with its cost.",
        &[
            (
                "on",
                "idle sessions' caches are kept for the time below, then lapse.",
            ),
            (
                "off",
                "caches lapse 5 minutes (or an hour) after the last request.",
            ),
        ],
        set(|m, v| config_mut(m).cache_keep.on = v == "on"),
    )];
    if k.on {
        rows.push(numbers(
            "  for",
            k.minutes as f64,
            "How long after a session goes idle its cache is kept. A 5-minute cache kept 15 minutes takes 3 small requests, about 30% of the context's price, to save a rewrite of 125% of it; an hour's cache is left alone unless this is longer than an hour.",
            &[0.0, 5.0, 30.0, 60.0, 120.0],
            "15 minutes",
            &|x| format!("{x:.0} minutes"),
            |m, v| config_mut(m).cache_keep.minutes = v as i64,
        ));
        rows.push(numbers(
            "  compact long waits",
            k.compact_after as f64,
            "Main conversations only, where the harness compacts itself (/compact). A session idle so long that its cache would lapse at about this time is compacted once, while its cache is still warm, rather than let go: the compaction reads the cached context and writes a short summary, so your next message writes the summary, not the whole context. It costs a turn whether you come back or not, and the agent may read again what the summary left out. Never one working, with a decision waiting on you or something running in its background, one whose cache has lapsed already, or one smaller than the smallest worth keeping; not again until it has had a turn of its own. An hour's cache (Claude Code signed in) lapses at an hour idle; a 5-minute cache lasts that long only if it's kept warm that long. Each is in the session's events as cache-compacted.",
            &[0.0, 60.0, 120.0],
            "off",
            &|x| {
                if x >= 120.0 {
                    "after 2 hours".into()
                } else {
                    "after 1 hour".into()
                }
            },
            |m, v| config_mut(m).cache_keep.compact_after = v as i64,
        ));
    }
    rows.push(choice(
        "Keep waiting subagents' cache",
        on_off(k.subagents),
        "A subagent waiting on a long command (a test run, a build) past its cache's life pays to write its whole context again when the command returns. A minute before that, rush moves the command to the background, where it runs on, so the subagent makes its next request while its cache is still warm. No request of rush's own and no model call. Each is in the session's events as subagent-cache-kept. Read as a session starts.",
        &[
            (
                "on",
                "a subagent's long command moves to the background before its cache lapses.",
            ),
            ("off", "subagents' caches lapse while they wait."),
        ],
        set(|m, v| config_mut(m).cache_keep.subagents = v == "on"),
    ));
    if k.subagents {
        rows.push(choice(
            "  slow commands in background",
            on_off(k.ahead),
            "A subagent's shell command that took its cache's life less a minute or more the last time it ran in the project starts in the background, and the subagent is told to check on it in short waits. Each is in the session's events as subagent-cache-ahead. Read at every command.",
            &[
                (
                    "on",
                    "a command known to outlast the cache starts in the background.",
                ),
                (
                    "off",
                    "it starts as asked, and is moved once its cache is about to lapse.",
                ),
            ],
            set(|m, v| config_mut(m).cache_keep.ahead = v == "on"),
        ));
    }
    for kind in super::guard::subagent_cache_kinds(m) {
        let label = format!(
            "{} subagents' cache",
            super::common::reg().harness_label(&kind)
        );
        rows.push(super::guard::subagent_cache_row(m, &kind, &label));
    }
    rows.push(numbers(
        "Smallest worth keeping",
        k.min_tokens as f64,
        "No small request for an idle conversation, nor a move for a waiting subagent, when its context is smaller than this: its rewrite costs cents ($0.13 at 20k tokens on Opus at $5/M), and the agent's own instructions and tools are most of it.",
        &[0.0, 10_000.0, 50_000.0, 100_000.0],
        "20k tokens",
        &|x| format!("{:.0}k tokens", x / 1000.0),
        |m, v| config_mut(m).cache_keep.min_tokens = v as i64,
    ));
    rows.push(numbers(
        "Most refreshes per wait",
        k.refreshes as f64,
        "The most small requests one idle stretch may take, or moves one waiting subagent may have. Auto keeps them under half the rewrite they save: 5 for a 5-minute cache, 9 for an hour's. Either way none goes once they'd cost more than the rewrite.",
        &[0.0, 2.0, 3.0, 5.0, 9.0],
        "auto",
        &|x| format!("{x:.0}"),
        |m, v| config_mut(m).cache_keep.refreshes = v as i64,
    ));
    vec![Section {
        title: "Prompt cache".into(),
        note: "on unless you turn it off; Claude Code for now".into(),
        rows,
        ..Section::default()
    }]
}

/// The context guard's settings as the page last read them; its defaults
/// before it did.
fn guard_read(m: &Model) -> GuardRead {
    dialog(m).guard.clone().unwrap_or_else(|| GuardRead {
        on: true,
        values: cg::manifest()
            .settings
            .into_iter()
            .map(|s| (s.key, s.default))
            .collect(),
    })
}

/// Keeps one of the context guard's settings, off the UI; the page shows
/// it at once.
fn set_guard(m: &mut Model, key: &str, v: &str) -> Out {
    let mut g = guard_read(m);
    g.values.insert(key.to_string(), v.to_string());
    dialog_mut(m).guard = Some(g);
    let (key, v) = (key.to_string(), v.to_string());
    m.later(
        move || rush_plugin::set_setting(cg::NAME, &key, &v),
        |m, r| {
            if let Err(e) = r {
                m.flash_err(&format!("{}: {e}", cg::NAME));
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// One of the context guard's rules, on or off.
fn guard_switch(
    g: &GuardRead,
    key: &'static str,
    label: &str,
    what: &str,
    on: &str,
    off: &str,
) -> Setting {
    let is_on = g.values.get(key).is_none_or(|v| v != "false");
    let mut st = choice(
        label,
        on_off(is_on),
        what,
        &[("on", on), ("off", off)],
        None,
    );
    st.run = Some(Rc::new(move |m: &mut Model, v: &str| {
        set_guard(m, key, if v == "on" { "true" } else { "false" })
    }));
    st
}

/// One of the context guard's numbers, from its choices, each named.
fn guard_number(
    g: &GuardRead,
    key: &'static str,
    label: &str,
    what: &str,
    choices: &[&str],
    name: &dyn Fn(u64) -> String,
) -> Setting {
    let value = g.values.get(key).cloned().unwrap_or_default();
    let mut st = Setting {
        label: label.into(),
        value,
        what: what.into(),
        ..Setting::default()
    };
    for c in choices {
        st.choices.push(c.to_string());
        st.names
            .insert(c.to_string(), name(c.parse().unwrap_or_default()));
        st.means.insert(c.to_string(), String::new());
    }
    st.run = Some(Rc::new(move |m: &mut Model, v: &str| set_guard(m, key, v)));
    st
}

/// A count as words: 1,000.
fn thousands(n: u64) -> String {
    if n >= 1000 {
        format!("{},{:03}", n / 1000, n % 1000)
    } else {
        n.to_string()
    }
}

/// Context guard: the bundled context-guard plugin's rules and their
/// numbers. They're its own settings, the same values its hook reads at
/// every call, so a change reaches running sessions at once; Settings ›
/// Plugins › context-guard turns the plugin itself on or off.
pub fn context_guard_section(m: &Model) -> Section {
    let g = guard_read(m);
    let on = |k: &str| g.values.get(k).is_none_or(|v| v != "false");
    let mut rows = vec![guard_switch(
        &g,
        cg::REREAD,
        "Don't re-read unchanged files",
        "Refuses an agent's read of a file it already read (the same lines or more) when the file hasn't changed since and it hasn't edited it; after its context is compacted it may read it again. Images, PDFs and notebooks are left alone. Each refusal is in the session's events as hook-denied.",
        "a read of a file unchanged since is refused, with when it was read.",
        "files are read again as asked.",
    )];
    if on(cg::REREAD) {
        rows.push(guard_number(
            &g,
            cg::REREAD_KB,
            "  smallest file it applies to",
            "A file under this size is read again without a word: refusing costs about what reading it does.",
            cg::REREAD_KB_CHOICES,
            &|kb| format!("{kb} KB"),
        ));
    }
    rows.push(guard_switch(
        &g,
        cg::BIG_READ,
        "Don't read huge files whole",
        "Refuses, once per file, a whole read of a text file past the size below, and points the agent to rg -n and to reading part of it; asked again, it reads it all. Images, PDFs and notebooks are left alone.",
        "a whole read of a huge file is refused once.",
        "files are read whole as asked.",
    ));
    if on(cg::BIG_READ) {
        rows.push(guard_number(
            &g,
            cg::BIG_LINES,
            "  huge means over",
            "How big a file is too big to read whole without a word first: over this many lines, or 48 KB for every 1,000 (24, 48 or 96 KB).",
            cg::BIG_LINES_CHOICES,
            &|n| format!("{} lines", thousands(n)),
        ));
    }
    rows.push(guard_switch(
        &g,
        cg::REPEAT,
        "Stop repeated failures",
        "Refuses a shell command that failed the same way (same exit, same end of its output) the number of times below in a row, with no edit in between, and tells the agent to change something first; asked again straight after, it runs.",
        "the next run of a command failing the same way is refused once.",
        "commands run however often they fail.",
    ));
    if on(cg::REPEAT) {
        rows.push(guard_number(
            &g,
            cg::REPEATS,
            "  after",
            "How many identical failures in a row before the next run is refused.",
            cg::REPEATS_CHOICES,
            &|n| format!("{n} in a row"),
        ));
    }
    rows.push(guard_switch(
        &g,
        cg::SQUEEZE,
        "Trim command output",
        "Takes terminal colours and codes, progress bars' redraws, spaces at line ends, runs of blank lines and repeated lines out of a shell command's output before the agent sees it, and cuts a long one to its first and last lines, the whole kept in a file it's pointed to. Never the output of cat, sed, git diff, rg and the like, whose exact spaces an edit needs. Each is in the session's events as hook-squeezed.",
        "command output is trimmed before the agent reads it.",
        "the agent reads command output as it came.",
    ));
    if on(cg::SQUEEZE) {
        rows.push(guard_number(
            &g,
            cg::SQUEEZE_LINES,
            "  longer than",
            "Output longer than this many lines keeps its first and last third (200 each of 600); the rest is left out, kept whole in a file the agent is pointed to.",
            cg::SQUEEZE_LINES_CHOICES,
            &|n| format!("{} lines", thousands(n)),
        ));
    }
    let note = if g.on {
        "on unless you turn it off; Claude Code for now"
    } else {
        "the context-guard plugin is off: Settings › Plugins turns it on"
    };
    Section {
        title: "Context guard".into(),
        note: note.into(),
        rows,
        ..Section::default()
    }
}

/// Advisor: a cheap model reads where your tokens go and says what would
/// save them.
pub fn advisor_section(m: &Model) -> Section {
    let advisor = choice(
        "Advisor",
        on_off(config(m).advisor),
        "Watches how you use your agents and says what would cut tokens or time. A cheap model (Haiku) reads a digest rush works out itself, at most every 3 hours, and an expensive one (Opus) checks the suggestions worth money against the transcripts before they reach you. It spends on your account; RUSH_ADVISOR decides it for one run.",
        &[
            ("off", "nothing runs."),
            ("on", "it looks over your agents' figures now and then."),
        ],
        set(|m, v| config_mut(m).advisor = v == "on"),
    );
    Section::new("Advisor", vec![advisor])
}
