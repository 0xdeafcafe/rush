//! The # commands the picker lists that no screen ran yet: #go, #clean,
//! #pin, #pr, #why, #worktree, #perm, #yolo, #away, #loop, #back
//! and #quit. Each acts on the agent picked in the list, or the open
//! Session's own (keys.go's command :874; the frame's command picks the
//! Session's agent). Port of internal/ui/keys.go:907-1110 (command's
//! cases), keepGoing keys.go:747, askStuck stucksubs.go:104, askClean and
//! askCleanAll tempwork.go:102-131, togglePin model.go:2695, openPR
//! (list_extras/listkeys.rs has it, and #stash) and ownWorktreeCommand cleanup.go:620.

use rush_core::agent::Agent;
use rush_fleet::Predicates;

use crate::frame::{Ext, Out};
use crate::model::Model;
use crate::style::*;

/// Adds the commands to x.
pub fn register(x: &mut Ext) {
    x.command(
        "go",
        "tell the agent to keep going (alt+g); after an error, to continue",
        go,
    );
    x.command(
        "clean",
        "delete the agent's temp work; all does every finished agent",
        clean,
    );
    x.command("pin", "pin or unpin the agent in its harness", pin);
    x.command("pr", "open the agent's pull request", pr);
    x.command(
        "why",
        "what rush did to this session in the last hour and why",
        why,
    );
    x.command(
        "worktree",
        "whether new sessions in this project get a worktree of their own",
        worktree,
    );
    x.command("perm", "choose session permissions", |m, arg| {
        perm(m, arg, false)
    });
    x.command("yolo", "this harness's bypass permission mode", |m, arg| {
        perm(m, arg, true)
    });
    x.command("away", "leave the session to carry on", |m, arg| {
        super::awaycmd::command(m, "away", arg)
    });
    x.command("loop", "keep the agent going while it's idle", |m, arg| {
        super::awaycmd::command(m, "loop", arg)
    });
    x.command("back", "what happened while you were away", |m, arg| {
        super::awaycmd::command(m, "back", arg)
    });
    x.command("quit", "leave rush", |_, _| Out::Quit);
}

/// The agent a command acts on: the list's pick, which is the open
/// Session's own while one is open.
fn target(m: &Model) -> Option<Agent> {
    m.selected().map(|i| m.agents[i].clone())
}

/// The agent, or says to pick one first (command's need).
fn need(m: &mut Model) -> Option<Agent> {
    let a = target(m);
    if a.is_none() {
        m.flash_err("select an agent first");
    }
    a
}

/// #go: tells an agent that stopped to go on, "keep going" after a
/// finished turn, "continue" after an error; a rush session silent too
/// long is asked what it waits on (keepGoing).
fn go(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else { return Out::Draw };
    let now = m.now;
    let name = a.display_name.clone();
    if a.interactive {
        m.flash_err(&format!("{name} is open in another terminal"));
    } else if a.stuck(now) && a.rush {
        ask_stuck(m, &a, a.quiet(now));
    } else if a.stuck(now) {
        let quiet = age(a.quiet(now));
        m.flash_err(&format!(
            "{name} has been silent {quiet} and takes nothing mid-turn · ctrl+x stops it"
        ));
    } else if a.job.live() || a.busy() {
        m.flash(&format!("{name} is still working"));
    } else {
        let t = a.continue_text().to_string();
        return crate::screens::session_core::cards_host::reply_to(m, &a.key, t.clone(), &t);
    }
    Out::Draw
}

/// Asks a rush session silent for quiet what it's waiting on, into its
/// turn without stopping it (askStuck).
fn ask_stuck(m: &mut Model, a: &Agent, quiet: rush_core::time::Ms) {
    use crate::screens::list_extras::stuck::{ask_done, ask_text};
    use crate::screens::session_core::host::{Op, hosts};
    let h = hosts(m);
    let (a, text) = (a.clone(), ask_text(quiet));
    m.later(
        move || {
            h.dial(&a)?.op(Op::Guide {
                text,
                images: Vec::new(),
            })?;
            Ok::<_, String>(a.display_name)
        },
        |m, r| {
            match r {
                Ok(name) => m.flash(&ask_done(&name)),
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
}

/// #clean [all]: Delete on the agent's temp work, or every finished
/// agent's (askClean, askCleanAll).
fn clean(m: &mut Model, arg: &str) -> Out {
    use crate::screens::projects_machine::delete::{ask_clean, ask_clean_all};
    if arg.trim() == "all" {
        ask_clean_all(m);
    } else if let Some(a) = need(m) {
        ask_clean(m, &a.key);
    }
    Out::Draw
}

/// #pin: pins or unpins the agent in its own list (togglePin), as
/// ctrl+t does.
fn pin(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else { return Out::Draw };
    crate::screens::accounts::onboard::did_step(m, "pin");
    crate::screens::list_extras::listkeys::toggle_pin(m, &a)
}

/// #pr: the agent's pull request in the browser, asking which first when
/// it has several (openPR), as ctrl+y does.
fn pr(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else { return Out::Draw };
    crate::screens::list_extras::listkeys::open_pr(m, Some(&a))
}

/// #why: what rush did to the open session it hosts (openInfo infoWhy).
/// It needs only the session's rush id, kept from its host, not a live
/// connection to it: what rush did is read from its log.
fn why(m: &mut Model, _: &str) -> Out {
    use crate::screens::session_core::open_key;
    let Some(a) = need(m) else { return Out::Draw };
    let ours = open_key(m).as_deref() == Some(a.key.as_str())
        && !crate::screens::accounts::info::rush_id(m, &a.key)
            .0
            .is_empty();
    if !ours {
        m.flash_err("only a session rush hosts has a why: open it in rush first");
        return Out::Draw;
    }
    crate::screens::accounts::info::open_info(m, crate::screens::accounts::info::WHY)
}

/// #worktree on|off: whether a session started from the box in this
/// project gets its own worktree; alone, it says which
/// (ownWorktreeCommand).
fn worktree(m: &mut Model, arg: &str) -> Out {
    use crate::screens::accounts::state::{cfg, cfg_mut, save};
    let dir = crate::screens::list_extras::prompt::start_dir(m);
    let project = rush_fleet::main_checkout(&dir);
    let name = std::path::Path::new(&project)
        .file_name()
        .map_or(project.clone(), |n| n.to_string_lossy().into_owned());
    match arg.trim() {
        a @ ("on" | "off") => {
            cfg_mut(m).own_worktrees.insert(project.clone(), a == "on");
            save(m);
        }
        "" => crate::screens::accounts::state::ensure_store(m),
        _ => {
            m.flash_err("#worktree on or #worktree off");
            return Out::Draw;
        }
    }
    if cfg(m).own_worktree(&project) {
        m.flash(&format!(
            "new sessions in {name} get a worktree and branch of their own · #worktree off"
        ));
    } else {
        m.flash(&format!(
            "new sessions in {name} work in the checkout itself · #worktree on"
        ));
    }
    Out::Draw
}

/// #perm and #yolo: on the open Session when it's the agent's, else the
/// next session's; an agent picked whose Session isn't open is asked to
/// be opened first (command's perm, yolo).
fn perm(m: &mut Model, arg: &str, yolo: bool) -> Out {
    use crate::screens::session_cards::permissions::perm_command;
    let open = crate::screens::session_core::open_key(m);
    match target(m) {
        Some(a) if open.as_deref() == Some(a.key.as_str()) => {
            perm_command(m, Some(&a.key), arg, yolo)
        }
        Some(_) => {
            m.flash_err("Open the session first to change its permissions");
            Out::Draw
        }
        None => perm_command(m, None, arg, yolo),
    }
}
