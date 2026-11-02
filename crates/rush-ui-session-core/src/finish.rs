//! What each reading of the list says happened to an agent since the one
//! before: it finished, it asks something, it started failing, its turn
//! stalled, each noted as a line from your agents (the feed's, kept to the
//! last 30). A turn that ends with agents it started still running in the
//! background hasn't finished: it's noted finished once they're done. A
//! usage limit rush moves a session on from by itself is only noted,
//! quietly. Clanker reacts to each: an error, an ask, one you kept waiting
//! moving on, one finishing; a stronger reaction takes over a weaker one
//! already running, never the other way. One that starts waiting on you,
//! stalls or stops on an error is a system notification too, unless
//! you're looking at it, rush is quiet or it follows another; the menu
//! bar posts the first kind itself when it runs. After Go's
//! internal/ui/model.go:1946-2033 (notify, with 4d29f271: Busy counts as
//! working; notifyLater), profiles.go:82 (limitHandled), clanker.go:429-552
//! (fxKind, clkFX, fxLen, react) and feed.go:21 noteEvent. clanker.rs
//! draws the reactions and ticks them on (clkFX.draw, tint, onFXTick).

use rush_core::agent::{Agent, State};
use rush_fleet::Predicates;

use crate::model::Model;

// What each reading said of each agent, and the lines from your agents,
// are the base's (crate::finish).
use crate::style::one_line;
use rush_ui_base::finish::note;
pub use rush_ui_base::finish::{Finish, note_event};

// What clanker reacts with is his (crate::clanker).
pub use crate::clanker::{FX_EVERY, Fx, Reaction, fx_tick, react};

/// A rush session a usage limit stopped that rush moves on by itself: its
/// profile doesn't wait, and a login, account or provider has room for
/// it. Nobody needs telling it stopped (limitHandled).
pub fn limit_handled(m: &Model, a: &Agent) -> bool {
    use crate::accounts as state;
    use rush_state::profiles::{LIMIT_HANDOFF, LIMIT_WAIT};
    use rush_ui_accounts::{autoswitch, keys, rows};
    let rate = a
        .spend
        .halt
        .as_ref()
        .is_some_and(|h| h.kind == "rate_limit");
    if !a.rush || !a.job.detail.starts_with("usage limit") && !rate {
        return false;
    }
    let p = state::cfg(m).profile_for(&a.job.cwd, &a.profile);
    if p.limit() == LIMIT_WAIT {
        return false;
    }
    let logins = &state::peek(m).logins;
    if a.kind.as_str() == rush_state::LOGINS_KIND && logins.len() > 1 {
        let now = m.clock.unwrap_or_else(rush_core::time::now);
        if autoswitch::next_login(logins, true, now).is_some() || keys::has_room(m) {
            return true;
        }
    }
    let room = rows::room(m);
    if p.pick_for(a.kind.as_str(), &room).1 {
        return true;
    }
    p.next(a.kind.as_str(), &room).is_some() && p.limit() == LIMIT_HANDOFF
}

/// Posts a notification off the UI thread: posting runs a program, and
/// whether the menu bar runs is a look at the disk. With unless_menu_bar,
/// the menu bar posts it instead when it runs (notifyLater).
fn notify_later(f: &mut Finish, title: String, body: String, unless_menu_bar: bool) {
    if rush_core::sandbox::active() {
        f.told.push((title, body));
        return;
    }
    std::thread::spawn(move || {
        let by = if unless_menu_bar && rush_menubar::running() {
            "menu bar"
        } else {
            rush_sys::actions::notify(&title, &body);
            "rush"
        };
        rush_sys::events::log(
            "notify",
            serde_json::json!({
                "title": title, "body": rush_host::events::clip(&body), "by": by,
            }),
        );
    });
}

/// With each reading of the list (notify).
pub fn on_fleet(m: &mut Model) {
    let now = m.clock.unwrap_or_else(rush_core::time::now);
    let mut f = std::mem::take(m.kept::<Finish>());
    let first = f.last_state.is_empty();
    // Not for the agent you're looking at while rush has focus.
    let looking = if m.term_focused {
        super::open_key(m).unwrap_or_default()
    } else {
        String::new()
    };
    let quiet = crate::accounts::cfg(m).quiet || m.following();
    for a in m.agents.iter().filter(|a| !a.checking) {
        let prev = f.last_state.get(&a.key).copied().unwrap_or("");
        // Agents it left running in the background are work: it finishes
        // when they do, not when its turn ends.
        let st = if a.busy() {
            "working"
        } else {
            a.job.state.as_str()
        };
        f.last_state.insert(a.key.clone(), st);
        let detail = a.job.detail.as_str();
        let failing =
            detail.starts_with("API error") || detail.starts_with("usage limit") || a.halted();
        let was_failing = f.last_err.insert(a.key.clone(), failing).unwrap_or(false);
        // A limit rush moves it on from is only noted, quietly, in the feed.
        let handled = failing && !was_failing && limit_handled(m, a);
        if !first && !prev.is_empty() {
            if handled {
                note(&mut f, a, now, "↻", "usage limit · rush moves it on".into());
            } else if failing && !was_failing {
                react(&mut f.fx, Fx::Error);
                note(&mut f, a, now, "✗", one_line(&a.halt_reason(now)));
            } else if prev != "blocked" && a.needs_you() {
                react(&mut f.fx, Fx::Ask);
                note(&mut f, a, now, "?", one_line(&a.job.needs));
            } else if prev == "blocked" && st == State::Working.as_str() {
                react(&mut f.fx, Fx::Answered);
            } else if prev == "working" && st == State::Done.as_str() {
                react(&mut f.fx, Fx::Done);
                note(&mut f, a, now, "✓", "finished".into());
            }
        }
        let watching = a.key == looking;
        // The menu bar icon, when it runs, notifies instead, with buttons.
        if !first && !quiet && !prev.is_empty() && prev != "blocked" && a.needs_you() && !watching {
            let body = if a.job.needs.is_empty() {
                one_line(&a.job.detail)
            } else {
                a.job.needs.clone()
            };
            // The same decisions asked again, by a turn that came between
            // (news of a pull request it follows, say), were said already.
            let k = &a.decisions_key;
            if k.is_empty() || f.told_card.get(&a.key) != Some(k) {
                if !k.is_empty() {
                    f.told_card.insert(a.key.clone(), k.clone());
                }
                notify_later(&mut f, format!("{} needs you", a.display_name), body, true);
            }
        }
        // A stalled turn waits for good: rush never ends it, so it says
        // so, once a stall.
        let was = f.last_stall.insert(a.key.clone(), a.stalled).unwrap_or(0);
        if a.stalled != 0 && a.stalled != was && !first && !prev.is_empty() {
            note(
                &mut f,
                a,
                now,
                "⚠",
                "stalled · nothing from the model, no step running".into(),
            );
            if !quiet && !watching {
                notify_later(
                    &mut f,
                    format!("{} stalled", a.display_name),
                    format!(
                        "nothing from the model for {}m and no step running · it keeps waiting until you stop it or send it something",
                        (now - a.stalled).max(0) / 60_000
                    ),
                    false,
                );
            }
        }
        // A turn that died on an error goes nowhere until someone says so;
        // the menu bar only knows about questions, so this is said here.
        if !first
            && !quiet
            && !prev.is_empty()
            && a.halted()
            && !was_failing
            && !watching
            && !handled
        {
            notify_later(
                &mut f,
                format!("{} stopped", a.display_name),
                a.halt_reason(now),
                false,
            );
        }
    }
    *m.kept::<Finish>() = f;
}
