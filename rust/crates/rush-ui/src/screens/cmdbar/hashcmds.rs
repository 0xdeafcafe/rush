//! The # commands the picker lists that no screen ran yet: #go, #clean,
//! #pin, #pr, #why, #stash, #worktree, #perm, #yolo, #away, #loop, #back
//! and #quit. Each acts on the agent picked in the list, or the open
//! Session's own (keys.go's command :874; the frame's command picks the
//! Session's agent). Port of internal/ui/keys.go:907-1110 (command's
//! cases), keepGoing keys.go:747, askStuck stucksubs.go:104, askClean and
//! askCleanAll tempwork.go:102-131, togglePin model.go:2695, openPR and
//! its picker picker.go:286 and :444-466, browse picker.go:302,
//! stashCommand stash.go:150 and ownWorktreeCommand cleanup.go:620.

use rush_core::agent::Agent;
use rush_core::text::Line;
use rush_fleet::Predicates;

use crate::frame::picker::{Pick, Picker};
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
        "stash",
        "what you set aside, sent, cleared and replaced",
        stash,
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

/// #pin: pins or unpins the agent in its own list, off the UI thread
/// (togglePin).
fn pin(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else { return Out::Draw };
    crate::screens::accounts::onboard::did_step(m, "pin");
    let Some(ad) = m
        .reg
        .get(a.kind.as_str())
        .filter(|ad| ad.pinner().is_some())
    else {
        m.flash_err(&format!("{}'s agent keeps no pins", a.display_name));
        return Out::Draw;
    };
    let (p, id) = (a.acct.clone(), a.job.id.clone());
    m.later(
        move || {
            ad.pinner()
                .map_or(Ok(()), |pn| pn.toggle_pin(&p, &id))
                .map_err(|e| e.to_string())
        },
        |m, r| {
            if let Err(e) = r {
                m.fail(&e);
            }
            Out::Draw
        },
    );
    Out::Draw
}

/// #pr: the agent's pull request in the browser, asking which first when
/// it has several (openPR).
fn pr(m: &mut Model, _: &str) -> Out {
    let Some(a) = need(m) else { return Out::Draw };
    match a.prs.len() {
        0 => m.flash_err("no pull request found for this agent"),
        1 => browse(m, a.prs[0].url.clone()),
        _ => {
            let w = 100.min(m.w.saturating_sub(6));
            let rows = a
                .prs
                .iter()
                .map(|p| Pick {
                    label: pr_row(p, w),
                    value: p.url.clone(),
                    ..Pick::default()
                })
                .collect();
            let mut p = Picker::new(
                format!("Pull requests · {}", one_line(&a.display_name)),
                rows,
                Box::new(|m, pick, _| {
                    if let Some(p) = pick {
                        browse(m, p.value.clone());
                    }
                    Out::Draw
                }),
            );
            p.keys = vec![
                ("↑↓".into(), "choose".into()),
                ("enter".into(), "open in browser".into()),
                ("esc".into(), "close".into()),
            ];
            m.open_sheet(Box::new(p));
        }
    }
    Out::Draw
}

/// A pull request's row in the picker, coloured by how it stands
/// (pickerBody's prs).
fn pr_row(p: &rush_core::adapter::Pr, w: usize) -> Line {
    let col = if p.state == "MERGED" {
        BLUE
    } else if p.state == "CLOSED" || p.state == "unknown" {
        DIM
    } else if p.checks.failed > 0 {
        RED
    } else {
        GREEN
    };
    let c = &p.checks;
    let checks = if c.passed + c.failed + c.pending > 0 {
        format!("  {}✓ {}✗ {}…", c.passed, c.failed, c.pending)
    } else {
        String::new()
    };
    let title = if p.title.is_empty() {
        p.url.trim_start_matches("https://github.com/").to_string()
    } else {
        p.title.clone()
    };
    line([
        sp(format!("#{:<6}", p.number), col),
        plain(" "),
        sp(fit_str(&title, w.saturating_sub(40)), TEXT),
        plain("  "),
        sp(fit_str(&p.state.to_lowercase(), 8), DIM),
        sp(checks, FAINT),
    ])
}

/// Opens url in the browser off the UI thread, saying so (browse).
fn browse(m: &mut Model, url: String) {
    m.later(
        {
            let url = url.clone();
            move || {
                std::process::Command::new("open")
                    .arg(&url)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map_err(|e| e.to_string())
                    .and_then(|s| {
                        if s.success() {
                            Ok(())
                        } else {
                            Err(s.to_string())
                        }
                    })
            }
        },
        move |m, r| {
            match r {
                Ok(()) => m.flash(&format!("opened {url}")),
                Err(e) => m.fail(&format!("couldn't open {url}: {e}")),
            }
            Out::Draw
        },
    );
}

/// #why: what rush did to the open session it hosts (openInfo infoWhy).
fn why(m: &mut Model, _: &str) -> Out {
    use crate::screens::session_core::{conn, open_key};
    let Some(a) = need(m) else { return Out::Draw };
    let hosted = open_key(m).as_deref() == Some(a.key.as_str()) && conn(m, &a.key).hosted();
    if !hosted {
        m.flash_err("only a session rush hosts has a why: open it in rush first");
        return Out::Draw;
    }
    crate::screens::accounts::info::open_info(m, crate::screens::accounts::info::WHY)
}

/// #stash: the stash's history, for the box with the keys
/// (stashCommand("history")).
fn stash(m: &mut Model, _: &str) -> Out {
    use crate::screens::plugin_ui::STASH_PLUGIN;
    use crate::screens::plugin_ui::hooks::{box_state, ui_session};
    let h = m.hooks();
    let called = h.as_ref().and_then(|h| {
        h.state()
            .plugins
            .iter()
            .find(|p| p.name == STASH_PLUGIN)
            .map(|p| p.values.get("called").cloned().unwrap_or_default())
    });
    let (Some(h), Some(called)) = (h, called) else {
        let w = rush_bundled::drafts::words_for("");
        m.flash_err(&format!(
            "{} isn't running · Settings, Plugins has it, as {STASH_PLUGIN}",
            w.title
        ));
        return Out::Draw;
    };
    let title = rush_bundled::drafts::words_for(&called).title;
    let s = target(m).map(|a| ui_session(&a));
    let (input, who) = match box_state(m) {
        Some((b, who)) => (Some(b), who),
        None => (None, String::new()),
    };
    m.later(
        move || h.command(STASH_PLUGIN, "history", s.as_ref(), &who, input.as_ref()),
        move |m, r| {
            if let Err(e) = r {
                m.fail(&format!("{title}: {e}"));
            }
            Out::Draw
        },
    );
    Out::Draw
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
