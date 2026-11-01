//! The card a session sits on when its sign-in ran out: the accounts it
//! could carry on as, each with its provider and how much room it has,
//! and the one picked carries it on. After Go's
//! internal/ui/signedout.go (signinChoice :24, signinChoices :32,
//! signinUsage :63, signedOutRows :79, signinKey :143) and
//! rushmode.go:2170 (onBg), :5935 (cardHint). The session's own part
//! (whether it's signed out, restarting its harness) is the hosted
//! sessions' screens': they draw this card with [`signed_out_rows`] and
//! run what [`signin_key`] says.

use rush_core::adapter::Feature;
use rush_core::text::{Line, Span};
use rush_core::time::Ms;

use super::bits::spread;
use super::look::look_of;
use super::rows::{AcctRow, account_rows, quota_window_name};
use super::setup::StartOver;
use crate::model::Model;
use crate::style::*;

/// An account a signed-out session can carry on as: the login it was on
/// (once you've logged in again), another of its harness's, or another
/// harness's, which takes the conversation over (signinChoice).
#[derive(Clone, Debug, PartialEq)]
pub struct SigninChoice {
    pub r: AcctRow,
    pub o: StartOver,
    /// The account it's on now.
    pub here: bool,
}

/// Where the card is: the account picked, and whether it has the keys.
#[derive(Clone, Copy, Debug, Default)]
pub struct Card {
    pub pick: usize,
    pub focus: bool,
    /// Drawn in a modal, not as a card on its own ground.
    pub in_modal: bool,
}

/// What the card's key does (signinKey's commands).
#[derive(Clone, Debug, PartialEq)]
pub enum Signin {
    /// Carry on as the login in use (continueSignedIn).
    Continue,
    /// Switch agent kind to account, then carry on.
    SwitchThenContinue { kind: String, account: String },
    /// Hand the conversation to o, with "continue".
    HandOver(StartOver),
}

/// Every account a session running as from could carry on as, the one
/// it's on first (signinChoices).
pub fn signin_choices(m: &Model, from: &StartOver) -> Vec<SigninChoice> {
    let rows = account_rows(m);
    let (mut here, mut rest) = (Vec::new(), Vec::new());
    for (i, r) in rows.iter().enumerate() {
        if r.head && rows.get(i + 1).is_some_and(|n| n.kind == r.kind && !n.head) {
            continue; // its accounts are listed: the agent's own line says nothing more
        }
        let same = r.kind == from.kind;
        if !same && !m.reg.supports(&r.kind, Feature::HandoffIn) {
            continue;
        }
        let mut o = if same {
            from.clone()
        } else {
            StartOver {
                kind: r.kind.clone(),
                ..StartOver::default()
            }
        };
        o.account = if r.head { String::new() } else { r.name(m) };
        let ch = SigninChoice {
            r: r.clone(),
            o,
            here: same && (r.head || r.current),
        };
        if ch.here {
            here.push(ch);
        } else {
            rest.push(ch);
        }
    }
    here.extend(rest);
    here
}

/// How full an account's tightest window is, coloured as it fills; None
/// with no reading (signinUsage).
pub fn signin_usage(r: &AcctRow, now: Ms) -> Option<Span> {
    let w = r.q.since(now).tightest("")?;
    let s = format!("{:.0}% · {}", w.percent, quota_window_name(&w));
    Some(if w.percent >= 95.0 {
        sp(s, RED)
    } else if w.percent >= 80.0 {
        sp(s, YELLOW)
    } else {
        sp(s, SUB)
    })
}

/// The last line of a card: its keys when it has focus, how to give it
/// focus when not (cardHint).
pub fn card_hint(c: &Card, keys: Line) -> Line {
    if c.focus {
        let mut l = line([sp("▸ ", ORANGE)]);
        join(&mut l, keys);
        if !c.in_modal {
            l.spans.push(sp("   ·   esc back to typing", DIM));
        }
        return l;
    }
    keys
}

/// The card a signed-out session waits on, w wide: who is the account's
/// name, error what stopped it (without host.AuthStopped), from what the
/// session runs as (signedOutRows).
pub fn signed_out_rows(
    m: &Model,
    who: &str,
    error: &str,
    from: &StartOver,
    c: &mut Card,
    now: Ms,
    w: usize,
) -> Vec<Line> {
    let bar = if c.in_modal {
        plain(" ")
    } else {
        sp("▍", RED)
    };
    let mut out = Vec::new();
    let in_modal = c.in_modal;
    let mut cl = |l: Line| {
        if in_modal {
            out.push(fit(l, w));
        } else {
            out.push(on_ground(l, w, CHROME_BG));
        }
    };
    let who = if who.is_empty() {
        Line::new()
    } else {
        line([sp(who, SUB), plain("  ")])
    };
    cl(spread(
        line([
            bar.clone(),
            plain(" "),
            sp("✗ ", RED),
            spb("Signed out", TEXT),
        ]),
        who,
        w,
    ));
    for row in wrap(error, 8.max(w.saturating_sub(6))) {
        cl(line([bar.clone(), plain("     "), sp(row, SUB)]));
    }
    let chs = signin_choices(m, from);
    c.pick = c.pick.min(chs.len().saturating_sub(1));
    if !chs.is_empty() {
        cl(line([bar.clone(), plain("     "), sp("Carry on as:", DIM)]));
    }
    // Six at a time, keeping the picked one in sight.
    let lo = (c.pick as isize - 2).min(chs.len() as isize - 6).max(0) as usize;
    for (i, ch) in chs.iter().enumerate().skip(lo).take(6) {
        let mark = if i == c.pick && c.focus {
            sp("▸ ", ORANGE)
        } else {
            plain("  ")
        };
        let l = look_of(&m.reg, &ch.r.kind);
        let mut name = line([sp(ch.r.label(m), TEXT)]);
        let e = ch.r.email();
        if !e.is_empty() && e != ch.r.name(m) {
            name.spans.push(sp(format!("  {e}"), DIM));
        }
        let mut notes: Vec<Span> = Vec::new();
        if let Some(u) = signin_usage(&ch.r, now) {
            notes.push(u);
        }
        if ch.here {
            notes.push(sp("in use, logged in again", GREEN));
        } else if ch.o.kind != from.kind {
            notes.push(sp("takes the conversation over", DIM));
        }
        let mut right = Line::new();
        for (j, n) in notes.into_iter().enumerate() {
            if j > 0 {
                right.spans.push(sp(" · ", DIM));
            }
            right.spans.push(n);
        }
        right.spans.push(plain("  "));
        let mut left = line([
            bar.clone(),
            plain("   "),
            mark,
            sp(l.glyph, l.colour()),
            plain(" "),
        ]);
        join(&mut left, name);
        cl(spread(left, right, w));
    }
    let edge = if c.focus && !c.in_modal {
        sp("▍", ORANGE)
    } else {
        bar
    };
    let k = |key: &str, label: &str| line([spb(key, TEXT), plain(" "), sp(label, SUB)]);
    let keys = if chs.len() > 1 {
        let mut l = k("↑↓", "pick");
        l.spans.push(plain("   "));
        join(&mut l, k("enter", "carry on as it"));
        l
    } else {
        k("y", "continue")
    };
    let mut l = line([edge, plain("   ")]);
    join(&mut l, card_hint(c, keys));
    cl(l);
    out
}

/// The signed-out card's keys: ↑↓ through its accounts, and enter (or y)
/// carries on as the one picked. None when the key isn't the card's: off
/// its ends ↑↓ go on as any card's (signinKey).
pub fn signin_key(
    m: &Model,
    from: &StartOver,
    c: &mut Card,
    s: &str,
    direct: bool,
) -> Option<Option<Signin>> {
    let chs = signin_choices(m, from);
    match s {
        "up" | "k" if c.focus && c.pick > 0 => {
            c.pick -= 1;
            Some(None)
        }
        "down" | "j" if c.focus && c.pick + 1 < chs.len() => {
            c.pick += 1;
            Some(None)
        }
        _ if s == "alt+y" || direct && s == "y" || c.focus && s == "enter" => {
            let Some(ch) = chs.get(c.pick).filter(|ch| !ch.here) else {
                return Some(Some(Signin::Continue));
            };
            if ch.o.kind == from.kind {
                // Its own harness's: the login switches, then it carries on.
                return Some(Some(Signin::SwitchThenContinue {
                    kind: ch.r.kind.clone(),
                    account: ch.o.account.clone(),
                }));
            }
            Some(Some(Signin::HandOver(ch.o.clone())))
        }
        _ => None,
    }
}
