//! The write lease: one participant of a conversation changes files at a
//! time. A participant's first file-changing call in a turn takes it
//! (lease.json in the lead's folder: who, and for which of its turns); it
//! is held until that turn ends, and dies with its holder: a holder whose
//! host is gone, or that is past that turn or at rest, holds nothing.
//! Another participant's file-changing call waits for it (the host holds
//! the approval, Claude Code's hook waits: server/guests.rs, hook.rs).
//! Rust-only.

use std::path::PathBuf;

use rush_core::tool::{Call, Kind};
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::paths::dir;

/// Who changes files in a conversation now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lease {
    /// The holder's session id.
    pub holder: String,
    /// Its name in the chat, for those waiting: "Kimi".
    pub name: String,
    /// The holder's turn it was taken in (Info.turn).
    pub turn: i64,
    pub at: Time,
}

/// What asking for the lease came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Took {
    /// It's the asker's now (or was already).
    Mine,
    /// Another holds it.
    Held(Lease),
}

/// Where lead's conversation keeps its lease.
pub fn path(lead: &str) -> PathBuf {
    dir(lead).join("lease.json")
}

/// What a participant me in its turn finds of the lease have, given
/// whether have's holder still holds it.
pub fn decide(have: Option<&Lease>, me: &str, holds: impl Fn(&Lease) -> bool) -> Option<Lease> {
    match have {
        Some(l) if l.holder != me && holds(l) => Some(l.clone()),
        _ => None,
    }
}

/// Whether l's holder still holds it: its host runs, and it's in the turn
/// it took it in, working or waiting on an answer.
pub fn holds(l: &Lease) -> bool {
    let Ok(i) = crate::client::read_info(&l.holder) else {
        return false;
    };
    crate::alive(i.host_pid)
        && i.turn == l.turn
        && matches!(i.state.as_str(), "working" | "blocked")
}

/// Asks for lead's lease for participant me (name, in its turn), taking it
/// when it's free or its holder no longer holds it.
pub fn take(lead: &str, me: &str, name: &str, turn: i64) -> std::io::Result<Took> {
    take_with(lead, me, name, turn, holds)
}

pub(crate) fn take_with(
    lead: &str,
    me: &str,
    name: &str,
    turn: i64,
    holds: impl Fn(&Lease) -> bool,
) -> std::io::Result<Took> {
    let p = path(lead);
    let _l = super::roster::lock(&p)?;
    let have: Option<Lease> = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(l) = decide(have.as_ref(), me, holds) {
        return Ok(Took::Held(l));
    }
    if have
        .as_ref()
        .is_none_or(|l| l.holder != me || l.turn != turn)
    {
        let l = Lease {
            holder: me.into(),
            name: name.into(),
            turn,
            at: Time::now(),
        };
        let b = serde_json::to_vec(&l).map_err(std::io::Error::other)?;
        crate::json::write_private(&p, &b)?;
    }
    Ok(Took::Mine)
}

/// Who holds lead's lease now, if anyone still does.
pub fn holder(lead: &str) -> Option<Lease> {
    let l: Lease = serde_json::from_slice(&std::fs::read(path(lead)).ok()?).ok()?;
    holds(&l).then_some(l)
}

/// Gives lead's lease up, if me holds it.
pub fn release(lead: &str, me: &str) {
    let p = path(lead);
    let Ok(_l) = super::roster::lock(&p) else {
        return;
    };
    let mine = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Lease>(&b).ok())
        .is_some_and(|l| l.holder == me);
    if mine {
        let _ = std::fs::remove_file(&p);
    }
}

/// Whether call changes files, run in cwd: an edit, write, delete, move or
/// notebook edit, or a shell command that does (shell_changes).
pub fn changes_files(call: &Call, cwd: &str) -> bool {
    match call.kind {
        Kind::Edit | Kind::Write | Kind::Delete | Kind::Move | Kind::Notebook => true,
        Kind::Shell => {
            let at = if call.input.cwd.is_empty() {
                cwd
            } else {
                &call.input.cwd
            };
            shell_changes(&call.input.command, at)
        }
        _ => false,
    }
}

/// The git commands that change the checkout.
const GIT_WRITES: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "clean",
    "commit",
    "merge",
    "mv",
    "pull",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "stash",
    "switch",
];

/// The programs that change files whatever they're given.
const WRITERS: &[&str] = &[
    "rm", "rmdir", "mkdir", "touch", "tee", "patch", "unlink", "truncate",
];

/// Whether shell command cmd changes files, run in dir: one that
/// rush_convo's shell_writes says writes a file, or a program that only
/// changes files, or a git command that changes the checkout.
pub fn shell_changes(cmd: &str, dir: &str) -> bool {
    if cmd.trim().is_empty() {
        return false;
    }
    if !rush_convo::shell_writes(cmd, dir).is_empty() {
        return true;
    }
    let mut s = cmd.to_string();
    for sep in ["||", ";", "|", "\n", "(", ")"] {
        s = s.replace(sep, "&&");
    }
    s.split("&&").any(|seg| {
        let mut w = seg.split_whitespace().skip_while(|w| {
            w.contains('=') || matches!(*w, "sudo" | "env" | "time" | "command" | "exec")
        });
        let Some(prog) = w.next() else {
            return false;
        };
        let prog = prog.rsplit('/').next().unwrap_or(prog);
        if WRITERS.contains(&prog) {
            return true;
        }
        if prog != "git" {
            return false;
        }
        // Past git's own flags (-C dir, -c k=v).
        let mut sub = None;
        let mut skip = false;
        for a in w {
            if skip {
                skip = false;
                continue;
            }
            if a == "-C" || a == "-c" {
                skip = true;
                continue;
            }
            if a.starts_with('-') {
                continue;
            }
            sub = Some(a);
            break;
        }
        sub.is_some_and(|s| GIT_WRITES.contains(&s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(holder: &str, turn: i64) -> Lease {
        Lease {
            holder: holder.into(),
            name: holder.to_uppercase(),
            turn,
            at: Time::default(),
        }
    }

    #[test]
    fn who_waits() {
        struct Case {
            name: &'static str,
            have: Option<Lease>,
            me: &'static str,
            alive: bool,
            waits: bool,
        }
        let cases = [
            Case {
                name: "free",
                have: None,
                me: "a",
                alive: true,
                waits: false,
            },
            Case {
                name: "mine",
                have: Some(lease("a", 3)),
                me: "a",
                alive: true,
                waits: false,
            },
            Case {
                name: "mine, an older turn",
                have: Some(lease("a", 2)),
                me: "a",
                alive: false,
                waits: false,
            },
            Case {
                name: "another's, held",
                have: Some(lease("b", 1)),
                me: "a",
                alive: true,
                waits: true,
            },
            Case {
                name: "another's, its holder gone",
                have: Some(lease("b", 1)),
                me: "a",
                alive: false,
                waits: false,
            },
        ];
        for c in cases {
            let got = decide(c.have.as_ref(), c.me, |_| c.alive);
            assert_eq!(got.is_some(), c.waits, "{}", c.name);
        }
    }

    #[test]
    fn taken_held_and_freed() {
        let lead = "guests-lease-lead";
        let alive = std::cell::Cell::new(true);
        let holds = |_: &Lease| alive.get();
        assert_eq!(
            take_with(lead, "kimi1", "Kimi", 4, holds).unwrap(),
            Took::Mine
        );
        assert_eq!(
            take_with(lead, "kimi1", "Kimi", 4, holds).unwrap(),
            Took::Mine,
            "again in its turn"
        );
        match take_with(lead, "claude1", "Claude", 9, holds).unwrap() {
            Took::Held(l) => assert_eq!(
                (l.holder.as_str(), l.name.as_str(), l.turn),
                ("kimi1", "Kimi", 4)
            ),
            t => panic!("taken while held: {t:?}"),
        }
        // Its holder stopped, crashed or ended its turn: the lease is free.
        alive.set(false);
        assert_eq!(
            take_with(lead, "claude1", "Claude", 9, holds).unwrap(),
            Took::Mine
        );
        alive.set(true);
        assert!(matches!(
            take_with(lead, "kimi1", "Kimi", 5, holds).unwrap(),
            Took::Held(_)
        ));
        release(lead, "kimi1"); // not its: kept
        assert!(matches!(
            take_with(lead, "kimi1", "Kimi", 5, holds).unwrap(),
            Took::Held(_)
        ));
        release(lead, "claude1");
        assert_eq!(
            take_with(lead, "kimi1", "Kimi", 5, holds).unwrap(),
            Took::Mine
        );
        // A session with no host holds nothing.
        assert_eq!(holder(lead), None);
    }

    #[test]
    fn what_changes_files() {
        let call = |kind: Kind, cmd: &str| Call {
            kind,
            input: rush_core::tool::Input {
                command: cmd.into(),
                ..Default::default()
            },
            ..Call::default()
        };
        let cases = [
            (Kind::Edit, "", true),
            (Kind::Write, "", true),
            (Kind::Delete, "", true),
            (Kind::Move, "", true),
            (Kind::Notebook, "", true),
            (Kind::Read, "", false),
            (Kind::Search, "", false),
            (Kind::Glob, "", false),
            (Kind::Shell, "ls -la", false),
            (Kind::Shell, "cargo test -p rush-host", false),
            (Kind::Shell, "git status && git diff", false),
            (Kind::Shell, "git log --oneline -5", false),
            (Kind::Shell, "echo hi > out.txt", true),
            (Kind::Shell, "printf x | tee notes.txt", true),
            (Kind::Shell, "rm -rf target/tmp", true),
            (Kind::Shell, "cd crates && mkdir new", true),
            (Kind::Shell, "git commit -m wip", true),
            (Kind::Shell, "git -C ../other checkout main", true),
            (Kind::Shell, "FOO=1 git stash", true),
            (Kind::Shell, "", false),
        ];
        for (kind, cmd, want) in cases {
            assert_eq!(
                changes_files(&call(kind, cmd), "/src/app"),
                want,
                "{kind:?} {cmd:?}"
            );
        }
    }
}
