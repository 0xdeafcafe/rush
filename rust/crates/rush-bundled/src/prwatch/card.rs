//! The watch plugin draws its own tasks: a rush watch pr running in the
//! background reads as the pull request it waits on, its checks so far,
//! and whether it's only waiting or has a failure it will say at the end.
//! Ported from internal/bundled/prwatch/card.go:1-206, with the card's
//! shape from internal/convo/jobcard.go:8-41 (JobCard, Seg, Tone): Go
//! registers Card with the conversation module; here the screen asks
//! [`card_for`] of each running task.

use std::collections::HashMap;
use std::sync::Mutex;

use rush_state::Time;

use super::live::{Live, Until, live_for};

/// How a plugin has a task it knows drawn in place of the shell row: what
/// it's waiting on, how far it's got, in its own words. The plugin says
/// what each piece is (a Tone); rush picks the colours.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobCard {
    /// its mark, before the kind
    pub mark: String,
    /// the mark's
    pub tone: Tone,
    /// waiting: the mark turns, rush's spinner, in tone
    pub spin: bool,
    /// in a word, where the shell row says "shell"
    pub kind: String,
    /// after the kind
    pub body: Vec<Seg>,
    /// Ends the row, before the task's own state.
    pub right: Vec<Seg>,
    /// A line each under the row when the task is picked.
    pub detail: Vec<Vec<Seg>>,
}

/// A run of a card's text in one tone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seg {
    pub text: String,
    pub tone: Tone,
}

/// What a piece of a card says: how rush colours it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// the ordinary, secondary
    #[default]
    Sub,
    /// what to read first
    Text,
    /// what's there for reference
    Dim,
    /// punctuation
    Faint,
    /// passed
    Good,
    /// failed
    Bad,
    /// still going
    Wait,
    /// what it's waiting for
    Info,
}

fn seg(text: impl Into<String>, tone: Tone) -> Seg {
    Seg {
        text: text.into(),
        tone,
    }
}

/// How long a watch's record, once read, is drawn from: the dock draws
/// many times a second, and a watch asks GitHub at most every 15s.
const LIVES_FRESH_NS: i128 = 1_000_000_000;

#[derive(Clone, Default)]
struct LiveRead {
    l: Option<Live>,
    at: Time,
    /// a read is out
    reading: bool,
}

static LIVES: Mutex<Option<HashMap<String, LiveRead>>> = Mutex::new(None);

/// What task's record last said. The card is drawn on the UI's thread, so
/// the file is read in the background: a frame draws the last reading,
/// and the dock's next one the new.
fn live_of(task: &str, now: Time) -> Option<Live> {
    let mut g = LIVES.lock().unwrap_or_else(|e| e.into_inner());
    let lives = g.get_or_insert_with(HashMap::new);
    let got = lives.get(task).cloned();
    if let Some(r) = &got
        && (r.reading || now.sub_nanos(r.at) < LIVES_FRESH_NS)
    {
        return r.l.clone();
    }
    lives.retain(|_, r| r.reading || now.sub_nanos(r.at) <= 60 * 1_000_000_000);
    let mut r = got.unwrap_or(LiveRead {
        at: now,
        ..Default::default()
    });
    r.reading = true;
    lives.insert(task.to_string(), r.clone());
    let task = task.to_string();
    std::thread::spawn(move || {
        let l = live_for(&task, now);
        let mut g = LIVES.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashMap::new).insert(
            task,
            LiveRead {
                l,
                at: now,
                reading: false,
            },
        );
    });
    r.l
}

/// The watch's card for a running task of kind ("shell", …) and id, when
/// it's a rush watch pr.
pub fn card_for(kind: &str, id: &str, now: Time) -> Option<JobCard> {
    if kind != "shell" {
        return None;
    }
    live_of(id, now).map(|l| card(&l, now))
}

pub(crate) fn card(l: &Live, now: Time) -> JobCard {
    let mut c = JobCard {
        mark: "◷".into(),
        tone: Tone::Wait,
        spin: true,
        kind: "watch".into(),
        ..Default::default()
    };
    c.body.push(seg(format!("#{}", l.number), Tone::Text));
    if !l.title.is_empty() {
        c.body
            .push(seg(format!(" {}", clip(&l.title, 36)), Tone::Sub));
    }
    let total = l.passed + l.failed + l.pending + l.skipped;
    if total > 0 {
        c.body.push(seg("  ", Tone::Faint));
        c.body.extend(bar(l, 12));
        c.body
            .push(seg(format!(" ✓{}", l.passed + l.skipped), Tone::Good));
        if l.failed > 0 {
            c.body.push(seg(format!(" ✗{}", l.failed), Tone::Bad));
        }
        if l.pending > 0 {
            c.body.push(seg(format!(" ◌{}", l.pending), Tone::Wait));
        }
    }
    c.body.push(seg("  ", Tone::Faint));
    if l.asked.is_zero() {
        c.body.push(seg("asking GitHub…", Tone::Dim));
    } else if l.failed > 0 && !l.reports() && l.pending > 0 {
        // It has news it won't give until the rest finish.
        (c.mark, c.tone, c.spin) = ("✗".into(), Tone::Bad, false);
        c.body.push(seg(
            format!("✗ {} failed", names(&l.failed_checks, 2)),
            Tone::Bad,
        ));
        c.body.push(seg(
            format!(" · says so once {} more finish", l.pending),
            Tone::Dim,
        ));
    } else if l.limited {
        c.body.push(seg(
            "GitHub's rate limit is low: asking less often",
            Tone::Wait,
        ));
    } else if total == 0 {
        c.body.push(seg("no checks yet", Tone::Dim));
    } else if l.pending > 0 {
        c.body.push(seg(
            format!("waiting on {}", names(&l.running, 2)),
            Tone::Info,
        ));
    } else {
        c.body.push(seg("all finished", Tone::Good));
    }
    let waited = round(now.sub_nanos(l.start));
    c.right.push(seg(waited, Tone::Sub));
    if l.timeout > 0 {
        c.right
            .push(seg(format!(" of {}", round(l.timeout as i128)), Tone::Dim));
    }
    if !l.next.is_zero() {
        let inn = l.next.sub_nanos(now);
        if inn > 0 {
            c.right
                .push(seg(format!(" · next look {}", round(inn)), Tone::Dim));
        } else {
            c.right.push(seg(" · looking", Tone::Info));
        }
    }
    c.detail = detail(l, now);
    c
}

/// The checks as cells, n wide at most: passed, failed, running.
fn bar(l: &Live, n: i64) -> Vec<Seg> {
    let total = l.passed + l.failed + l.pending + l.skipped;
    let n = n.min(total);
    let cells = |k: i64| if k == 0 { 0 } else { (k * n / total).max(1) };
    let (bad, wait) = (cells(l.failed), cells(l.pending));
    let good = (n - bad - wait).max(0);
    [
        (good, Tone::Good, "━"),
        (bad, Tone::Bad, "━"),
        (wait, Tone::Faint, "─"),
    ]
    .into_iter()
    .filter(|(k, _, _)| *k > 0)
    .map(|(k, tone, r)| seg(r.repeat(k as usize), tone))
    .collect()
}

/// What the picked card says under its row.
fn detail(l: &Live, now: Time) -> Vec<Vec<Seg>> {
    let mut out = Vec::new();
    if !l.failed_checks.is_empty() {
        out.push(vec![
            seg("failed   ", Tone::Dim),
            seg(names(&l.failed_checks, 6), Tone::Bad),
        ]);
    }
    if !l.running.is_empty() {
        out.push(vec![
            seg("running  ", Tone::Dim),
            seg(names(&l.running, 6), Tone::Wait),
        ]);
    }
    let mut ends = match l.until.0.as_str() {
        Until::DONE => "once every check has finished",
        Until::FAIL => "at the first failure, else once all finish",
        Until::FIRST => "at the first check to finish",
        Until::EACH => "once all finish, a line as each does",
        _ => "",
    }
    .to_string();
    if l.timeout > 0 {
        let left = (l.timeout as i128 - now.sub_nanos(l.start)).max(0);
        ends.push_str(&format!(", or in {} at its timeout", round(left)));
    }
    out.push(vec![
        seg("ends     ", Tone::Dim),
        seg(format!("--until {}", l.until.0), Tone::Info),
        seg(format!(": {ends}"), Tone::Sub),
    ]);
    if !l.asked.is_zero() {
        out.push(vec![
            seg("asked    ", Tone::Dim),
            seg(
                format!(
                    "GitHub {} ago · {}#{}",
                    round(now.sub_nanos(l.asked)),
                    l.repo,
                    l.number
                ),
                Tone::Sub,
            ),
        ]);
    }
    out
}

/// The first n, and how many more.
fn names(all: &[String], n: usize) -> String {
    if all.len() <= n {
        return all.join(", ");
    }
    format!("{} +{}", all[..n].join(", "), all.len() - n)
}

fn clip(s: &str, n: usize) -> String {
    rush_plugin::clip(s, n)
}

/// A wait as the row has room for: 40s, 12m, 1h05m.
fn round(nanos: i128) -> String {
    let secs = nanos / 1_000_000_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    format!("{}h{:02}m", secs / 3600, secs / 60 % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(ss: &[Seg]) -> String {
        ss.iter().map(|s| s.text.as_str()).collect()
    }

    const MIN: i64 = 60 * 1_000_000_000;

    // card_test.go TestTheCardSaysAFailureTheWatchHasntReported
    #[test]
    fn the_card_says_a_failure_the_watch_hasnt_reported() {
        let now = Time::from_unix(10_000, 0);
        let l = Live {
            number: 8982,
            title: "fix(security): close the alerts".into(),
            until: Until(Until::DONE.into()),
            timeout: 110 * MIN,
            start: now.add_nanos(-66 * MIN),
            asked: now.add_nanos(-20_000_000_000),
            next: now.add_nanos(40_000_000_000),
            passed: 41,
            failed: 1,
            pending: 2,
            failed_checks: vec!["test".into()],
            running: vec!["codeql-complete".into(), "sdk-python-complete".into()],
            ..Default::default()
        };
        let c = card(&l, now);
        let body = text(&c.body);
        for want in [
            "#8982",
            "✓41",
            "✗1",
            "◌2",
            "test failed",
            "says so once 2 more finish",
        ] {
            assert!(body.contains(want), "body lacks {want:?}: {body}");
        }
        assert!(
            c.tone == Tone::Bad && !c.spin,
            "a watch holding a failure reads as waiting: {c:?}"
        );
        assert_eq!(text(&c.right), "1h06m of 1h50m · next look 40s");
        let d = text(&c.detail[2]);
        assert!(
            d.contains("--until done") && d.contains("in 44m at its timeout"),
            "ends line {d:?}"
        );
    }

    // card_test.go TestTheCardWaitsWhenNothingHasFailed
    #[test]
    fn the_card_waits_when_nothing_has_failed() {
        let now = Time::from_unix(10_000, 0);
        let c = card(
            &Live {
                number: 7,
                until: Until(Until::FAIL.into()),
                start: now,
                asked: now,
                passed: 3,
                pending: 1,
                running: vec!["e2e".into()],
                ..Default::default()
            },
            now,
        );
        assert!(c.spin && text(&c.body).contains("waiting on e2e"), "{c:?}");
        let c = card(
            &Live {
                number: 7,
                start: now,
                ..Default::default()
            },
            now,
        );
        assert!(text(&c.body).contains("asking GitHub"), "{}", text(&c.body));
    }
}
