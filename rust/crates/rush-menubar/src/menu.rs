//! What the app says, worked out apart from AppKit so it's tested on its
//! own: times, the answers a wait takes, tooltips, the icon's pose and
//! which notifications to post or take back. Port of the words and
//! choices in internal/menubar/app.swift:60-90 (ago, resets), 196-215
//! (draw), 351-395 (named, usageTip, actions), 432-470 (notifyWaiting,
//! notify) and 513-516 (level).

use std::collections::HashMap;

use rush_core::time::Ms;

use crate::wire::{Account, Op, Waiting, Window};

/// How long ago since was, in a few characters: "4m", "2h", "2h 5m", "3d";
/// "" for no time.
pub fn ago(since: Ms, now: Ms) -> String {
    if since == 0 {
        return String::new();
    }
    let m = ((now - since) / 60_000).max(0);
    if m < 60 {
        return format!("{m}m");
    }
    if m < 60 * 24 {
        return if m % 60 == 0 {
            format!("{}h", m / 60)
        } else {
            format!("{}h {}m", m / 60, m % 60)
        };
    }
    format!("{}d", m / 60 / 24)
}

/// A local time's parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    pub yday: i32,
    pub wday: i32,
    pub hour: i32,
    pub min: i32,
}

/// ms in this Mac's time zone.
pub fn local(ms: Ms) -> Local {
    let t: libc::time_t = (ms.div_euclid(1000)) as libc::time_t;
    // SAFETY: localtime_r fills tm from t; both are plain data here.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    Local {
        year: tm.tm_year,
        yday: tm.tm_yday,
        wday: tm.tm_wday,
        hour: tm.tm_hour,
        min: tm.tm_min,
    }
}

/// When a window resets: "14:05" today, "Thu 09:00" another day; "" for no
/// time.
pub fn resets(at: Ms, now: Ms) -> String {
    if at == 0 {
        return String::new();
    }
    resets_in(local(at), local(now))
}

pub(crate) fn resets_in(at: Local, now: Local) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let clock = format!("{:02}:{:02}", at.hour, at.min);
    if (at.year, at.yday) == (now.year, now.yday) {
        return clock;
    }
    format!("{} {clock}", DAYS[at.wday.clamp(0, 6) as usize])
}

/// What a wait's notification is matched by: the request it's for, else
/// what it needs.
pub fn sig(w: &Waiting) -> &str {
    if w.req.is_empty() { &w.needs } else { &w.req }
}

/// The answers a wait takes from outside rush, as button titles and the op
/// each sends.
pub fn actions(w: &Waiting) -> Vec<(String, Op)> {
    let op = |op: &str| Op {
        op: op.into(),
        key: w.key.clone(),
        req: w.req.clone(),
        ..Op::default()
    };
    match w.kind.as_str() {
        "question" => w
            .options
            .iter()
            .take(9)
            .map(|o| {
                (
                    o.clone(),
                    Op {
                        answer: o.clone(),
                        ..op("answer")
                    },
                )
            })
            .collect(),
        "permission" => vec![("Allow".into(), op("allow")), ("Deny".into(), op("deny"))],
        "limit" => vec![
            (
                "Continue when it resets".into(),
                Op {
                    yes: true,
                    ..op("limit")
                },
            ),
            ("Wait for me".into(), op("limit")),
        ],
        _ => vec![],
    }
}

/// A window's name for a sentence: "Weekly", "5-hour"; fallback when it
/// has none.
pub fn named(w: &Window, fallback: &str) -> String {
    let mut c = w.name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => fallback.into(),
    }
}

/// An account's tooltip: who, what's wrong, when its limits reset, and
/// how many of its agents run.
pub fn usage_tip(a: &Account, now: Ms) -> String {
    let mut t = Vec::new();
    if !a.email.is_empty() {
        t.push(a.email.clone());
    }
    if !a.problem.is_empty() {
        t.push(a.problem.clone());
    }
    for (w, fallback) in [(&a.five_hour, "5-hour"), (&a.seven_day, "Week")] {
        let r = resets(w.resets_at, now);
        if w.present && !r.is_empty() {
            t.push(format!("{} resets {r}", named(w, fallback)));
        }
    }
    if a.live > 0 {
        t.push(format!("{} running", a.live));
    }
    t.join("\n")
}

/// What the icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    Idle,
    /// Its cap off: on every other second while an agent works.
    Working,
    /// Orange: someone needs you.
    Needs,
}

/// The icon's pose at second secs.
pub fn pose(waiting: usize, working: usize, secs: i64) -> Pose {
    if waiting > 0 {
        Pose::Needs
    } else if working > 0 && secs / 2 % 2 == 1 {
        Pose::Working
    } else {
        Pose::Idle
    }
}

/// How full a window is, as a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Green,
    Orange,
    Red,
}

pub fn level(pct: f64) -> Level {
    if pct >= 90.0 {
        Level::Red
    } else if pct >= 70.0 {
        Level::Orange
    } else {
        Level::Green
    }
}

/// A window's percent as the bar says it: "40%", or "–" with none.
pub fn percent(w: &Window) -> String {
    if w.present {
        format!("{}%", w.percent.round() as i64)
    } else {
        "–".into()
    }
}

/// The notification category a wait's buttons go in: one per set of
/// buttons.
pub fn category(w: &Waiting, acts: &[(String, Op)]) -> String {
    let labels: Vec<&str> = acts.iter().map(|(l, _)| l.as_str()).collect();
    let text = if w.kind == "question" { ".text" } else { "" };
    format!("c.{}{text}", labels.join("\u{1f}"))
}

/// A wait's notification's subtitle.
pub fn subtitle(w: &Waiting) -> &str {
    if w.header.is_empty() {
        "needs you"
    } else {
        &w.header
    }
}

/// What to do with notifications on a new state: take back those answered
/// or gone, and post once for each new question, unless it's the first
/// state (what's waiting then isn't news) or it was seen in rush.
pub struct Notices<'a> {
    pub gone: Vec<String>,
    pub post: Vec<&'a Waiting>,
    pub now: HashMap<String, String>,
}

pub fn notices<'a>(
    notified: &HashMap<String, String>,
    waiting: &'a [Waiting],
    got: bool,
) -> Notices<'a> {
    let now: HashMap<String, String> = waiting
        .iter()
        .map(|w| (w.key.clone(), sig(w).to_string()))
        .collect();
    let mut gone: Vec<String> = notified
        .iter()
        .filter(|(k, v)| now.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    gone.sort();
    let post = waiting
        .iter()
        .filter(|w| notified.get(&w.key).map(String::as_str) != Some(sig(w)))
        .filter(|w| got && !w.seen)
        .collect();
    Notices { gone, post, now }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Ms = 60_000;

    #[test]
    fn ago_in_a_few_characters() {
        let now = 1_000 * 24 * 60 * MIN;
        for (since, want) in [
            (0, ""),
            (now - 4 * MIN, "4m"),
            (now + 5 * MIN, "0m"),
            (now - 120 * MIN, "2h"),
            (now - 125 * MIN, "2h 5m"),
            (now - 3 * 24 * 60 * MIN - 7, "3d"),
        ] {
            assert_eq!(ago(since, now), want, "{since}");
        }
    }

    #[test]
    fn resets_today_or_on_a_day() {
        let at = |yday, wday, hour, min| Local {
            year: 126,
            yday,
            wday,
            hour,
            min,
        };
        assert_eq!(resets_in(at(10, 4, 9, 5), at(10, 4, 8, 0)), "09:05");
        assert_eq!(resets_in(at(12, 6, 14, 30), at(10, 4, 8, 0)), "Sat 14:30");
        assert_eq!(resets(0, 5), "");
    }

    fn wait(kind: &str) -> Waiting {
        Waiting {
            key: "k".into(),
            req: "r".into(),
            kind: kind.into(),
            options: (1..=10).map(|i| format!("o{i}")).collect(),
            ..Waiting::default()
        }
    }

    #[test]
    fn answers_a_wait_takes() {
        let q = actions(&wait("question"));
        assert_eq!(q.len(), 9, "at most nine options");
        assert_eq!(q[0].0, "o1");
        assert_eq!(
            (
                q[0].1.op.as_str(),
                q[0].1.answer.as_str(),
                q[0].1.req.as_str()
            ),
            ("answer", "o1", "r")
        );
        let p: Vec<(String, String)> = actions(&wait("permission"))
            .into_iter()
            .map(|(l, o)| (l, o.op))
            .collect();
        assert_eq!(
            p,
            [
                ("Allow".into(), "allow".into()),
                ("Deny".into(), "deny".into())
            ]
        );
        let l = actions(&wait("limit"));
        assert_eq!(
            (l[0].0.as_str(), l[0].1.yes),
            ("Continue when it resets", true)
        );
        assert_eq!((l[1].0.as_str(), l[1].1.yes), ("Wait for me", false));
        assert!(actions(&wait("")).is_empty());
        assert_eq!(
            category(&wait("permission"), &actions(&wait("permission"))),
            "c.Allow\u{1f}Deny"
        );
        assert!(category(&wait("question"), &q).ends_with("o9.text"));
    }

    #[test]
    fn tips_and_names() {
        let w = |name: &str| Window {
            present: true,
            name: name.into(),
            ..Window::default()
        };
        assert_eq!(named(&w("weekly"), "Week"), "Weekly");
        assert_eq!(named(&w(""), "Week"), "Week");
        let a = Account {
            email: "me@x".into(),
            problem: "sign-in expired".into(),
            live: 2,
            five_hour: Window {
                resets_at: 0,
                ..w("5-hour")
            },
            ..Account::default()
        };
        assert_eq!(usage_tip(&a, 0), "me@x\nsign-in expired\n2 running");
        assert_eq!(
            percent(&Window {
                present: true,
                percent: 39.6,
                ..Window::default()
            }),
            "40%"
        );
        assert_eq!(percent(&Window::default()), "–");
        assert_eq!(level(95.0), Level::Red);
        assert_eq!(level(70.0), Level::Orange);
        assert_eq!(level(10.0), Level::Green);
    }

    #[test]
    fn poses() {
        assert_eq!(pose(1, 3, 3), Pose::Needs);
        assert_eq!(pose(0, 1, 1), Pose::Idle);
        assert_eq!(pose(0, 1, 2), Pose::Working);
        assert_eq!(pose(0, 1, 3), Pose::Working);
        assert_eq!(pose(0, 1, 4), Pose::Idle);
        assert_eq!(pose(0, 0, 3), Pose::Idle);
    }

    #[test]
    fn notifies_once_per_question() {
        let a = Waiting {
            key: "a".into(),
            needs: "asks".into(),
            ..Waiting::default()
        };
        let b = Waiting {
            key: "b".into(),
            req: "r1".into(),
            ..Waiting::default()
        };
        // The first state isn't news.
        let n = notices(&HashMap::new(), std::slice::from_ref(&a), false);
        assert!(n.post.is_empty() && n.gone.is_empty());
        let notified = n.now;
        // A new one is; one seen in rush isn't.
        let seen = Waiting {
            key: "c".into(),
            seen: true,
            ..Waiting::default()
        };
        let all = [a.clone(), b.clone(), seen];
        let n = notices(&notified, &all, true);
        assert_eq!(
            n.post.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
        // b asking something else takes back its notice and posts again;
        // a answered is taken back.
        let b2 = Waiting {
            req: "r2".into(),
            ..b
        };
        let n2 = notices(&n.now, std::slice::from_ref(&b2), true);
        assert_eq!(n2.gone, ["a", "b", "c"]);
        assert_eq!(n2.post.len(), 1);
    }
}
