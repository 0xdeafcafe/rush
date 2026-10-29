//! You can leave a session to carry on while you're away (on a flight, in
//! a meeting): until Away.until, each time it has sat idle at a check-in
//! it's told to keep going. What it asks or needs approved meanwhile is
//! refused at once, with why, and saved for you (Away.held), so it carries
//! on with what it can. The host keeps it, not a view, so it goes on with
//! rush closed, and stays up while it does. A loop is the same with you
//! there: told to keep going, while what it asks waits for you as ever.
//! Either ends once the agent says it's all done.
//!
//! Ported from internal/host/away.go:59-110 and 205-223 (the server's
//! share is in server/watch.rs).

use std::time::Duration;

use rush_state::Time;

use crate::wire::Away;

/// How often an away session is checked on.
pub const DEFAULT_AWAY_EVERY: Duration = Duration::from_secs(30 * 60);

/// What a question or permission asked while you're away is refused with.
pub const AWAY_HELD: &str = "The user is away, so nothing is answered or approved until they're back. This is saved for them; carry on with whatever doesn't depend on it, and don't ask it again.";

/// What happens to a at now: (over, send): over (until passed), or a
/// check-in to send (the session is idle and next has come). One busy at
/// next is told as soon as it goes idle.
pub fn away_due(a: Option<&Away>, idle: bool, now: Time) -> (bool, bool) {
    let Some(a) = a.filter(|a| a.on()) else {
        return (false, false);
    };
    if !a.until.is_zero() && !now.before(a.until) {
        return (true, false);
    }
    (false, idle && !now.before(a.next))
}

/// What a session is told at a check-in.
pub fn away_note(a: &Away, now: Time) -> String {
    let done = "If it's all done, start your reply with \"All done:\", say what you did in a line, and stop.";
    if a.r#loop {
        return format!("Rush check-in: keep going with what I asked. {done}");
    }
    let left = a.until.sub_nanos(now).max(60_000_000_000);
    let mins = ((left + 30_000_000_000) / 60_000_000_000) as i64; // to the minute
    let left = if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    };
    format!(
        "Rush check-in: I'm away for about another {left}. Nothing you ask or need approved is answered till I'm back: it's saved for me, so carry on with whatever doesn't depend on it, make reasonable calls yourself and note them. {done}"
    )
}

/// Whether a turn's last words are the "All done:" away_note asks for once
/// there's nothing left to do.
pub fn away_done(text: &str) -> bool {
    text.trim_start_matches([' ', '\n', '*', '_', '#'])
        .to_lowercase()
        .starts_with("all done")
}

/// Holds off the Mac's idle sleep until until, or the host ends: asleep,
/// nothing checks in. A closed lid sleeps all the same. Its pid, to end it
/// by. shortcut: macOS only (caffeinate), systemd-inhibit would do on
/// Linux.
pub fn keep_awake(until: Time) -> Option<i32> {
    let secs = until.sub_nanos(Time::now()) / 1_000_000_000 + 1;
    let mut c = std::process::Command::new("caffeinate")
        .args([
            "-i",
            "-w",
            &std::process::id().to_string(),
            "-t",
            &secs.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let pid = c.id() as i32;
    // Reaped once it ends, so it never lingers a zombie.
    let _ = std::thread::Builder::new()
        .name("host-awake".into())
        .spawn(move || {
            let _ = c.wait();
        });
    Some(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now: Time, mins: i64) -> Time {
        now.add_nanos(mins * 60_000_000_000)
    }

    // away_test.go:12
    #[test]
    fn due() {
        let now = Time::now();
        let a = Away {
            until: at(now, 60),
            every: Duration::from_secs(1800),
            next: now,
            ..Default::default()
        };
        assert_eq!(
            away_due(Some(&a), true, now),
            (false, true),
            "idle at a check-in is told to keep going"
        );
        assert!(!away_due(Some(&a), false, now).1, "busy is left alone");
        let later = Away {
            until: at(now, 60),
            next: at(now, 1),
            ..Default::default()
        };
        assert!(!away_due(Some(&later), true, now).1, "not yet due");
        assert!(away_due(Some(&a), true, at(now, 60)).0, "the time's up");
        assert_eq!(away_due(None, true, now), (false, false), "not away");
        let lp = Away {
            r#loop: true,
            next: now,
            ..Default::default()
        };
        assert_eq!(
            away_due(Some(&lp), true, at(now, 60_000)),
            (false, true),
            "a loop with no time goes on"
        );
        let ended = Away {
            until: at(now, 60),
            next: now,
            ended: now,
            ..Default::default()
        };
        assert!(
            !away_due(Some(&ended), true, now).1,
            "ended, it's still checked on"
        );
    }

    // away_test.go:38
    #[test]
    fn done_and_notes() {
        for (text, want) in [
            ("All done: tests pass", true),
            ("**All done.** Shipped", true),
            ("all done", true),
            ("Not all done yet", false),
            ("", false),
        ] {
            assert_eq!(away_done(text), want, "{text:?}");
        }
        let now = Time::now();
        let n = away_note(
            &Away {
                until: at(now, 134),
                ..Default::default()
            },
            now,
        );
        assert!(
            n.contains("another 2h14m.") && n.contains("saved for me"),
            "{n}"
        );
        let n = away_note(
            &Away {
                until: at(now, 180),
                ..Default::default()
            },
            now,
        );
        assert!(n.contains("another 3h00m."), "{n}");
        let n = away_note(
            &Away {
                r#loop: true,
                ..Default::default()
            },
            now,
        );
        assert!(!n.contains("away") && n.contains("All done:"), "{n}");
    }
}
