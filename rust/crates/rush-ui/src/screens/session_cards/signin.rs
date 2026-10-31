//! The signed-out card in the open Session: a session that stopped
//! because its sign-in ran out waits on a card of the accounts it could
//! carry on as; enter (or y) carries it on as the one picked. The card is
//! accounts::signedout's; this is the Session's side of it, after Go's
//! internal/ui/signedout.go (signedOut :16, signinKey :150,
//! continueSignedIn :178) and rushmode.go (cardRows :3106, cardKind
//! :5964, cardID :5981, cardKey's signin :6028).

use rush_core::text::Line;
use rush_host::server::AUTH_STOPPED;

use super::{Act, Conn, HostInfo, act};
use crate::frame::Out;
use crate::model::Model;
use crate::screens::accounts::setup::StartOver;
use crate::screens::accounts::signedout::{Signin, signed_out_rows, signin_key};

/// Whether the session sits stopped because its sign-in ran out, and no
/// continue has gone yet (signedOut).
pub fn signed_out(c: &Conn, i: &HostInfo) -> bool {
    !i.cold
        && i.error.starts_with(AUTH_STOPPED)
        && i.error != c.signin_sent
        && !matches!(i.state.as_str(), "working" | "blocked" | "starting")
}

/// What session key runs as now (sessionStart).
fn from(m: &Model, key: &str, i: &HostInfo) -> StartOver {
    crate::screens::bars::agent::session_start(m, key).unwrap_or_else(|| StartOver {
        kind: i.kind.clone(),
        ..StartOver::default()
    })
}

/// The card's rows, w wide (signedOutRows).
pub fn rows(m: &Model, key: &str, i: &HostInfo, c: &mut Conn, w: usize) -> Vec<Line> {
    let who = m
        .agents
        .iter()
        .find(|a| a.key == key)
        .map(|a| a.acct.name.clone())
        .unwrap_or_default();
    let error = i.error.strip_prefix(AUTH_STOPPED).unwrap_or(&i.error);
    let from = from(m, key, i);
    c.signin.focus = c.card_focus;
    let now = super::now(m);
    signed_out_rows(m, &who, error, &from, &mut c.signin, now, w)
}

/// The card's keys: ↑↓ through its accounts, off its ends as any card's,
/// and enter (or y) carries on as the one picked; direct when its letters
/// answer without ↑ (cardKey's signin, signinKey).
pub fn key(m: &mut Model, key: &str, i: &HostInfo, c: &mut Conn, s: &str, direct: bool) -> Out {
    let from = from(m, key, i);
    c.signin.focus = c.card_focus;
    match signin_key(m, &from, &mut c.signin, s, direct) {
        None => Out::Pass,
        Some(None) => Out::Draw, // moved through its accounts
        Some(Some(sig)) => {
            if c.card_focus {
                c.card_again = c.card_shown.clone();
            }
            c.card_focus = false;
            c.signin_sent = i.error.clone();
            carry_on(m, key, sig)
        }
    }
}

/// Carries the session on as the card says: the login in use, its own
/// harness's other login switched to first, or another harness's taking
/// the conversation over with "continue".
fn carry_on(m: &mut Model, key: &str, sig: Signin) -> Out {
    match sig {
        Signin::Continue => continue_signed_in(m, key),
        Signin::SwitchThenContinue { kind, account } => {
            let o = StartOver {
                kind,
                account,
                ..StartOver::default()
            };
            let k = key.to_string();
            crate::screens::session_core::start::switch_then(
                m,
                &o,
                Box::new(move |m| continue_signed_in(m, &k)),
            )
        }
        Signin::HandOver(o) => {
            crate::screens::session_core::switch::switch_session_message(m, key, o, "continue")
        }
    }
}

/// Its agent, which holds the old sign-in, stops, and a continue starts a
/// fresh one signed in as whoever is now (continueSignedIn).
fn continue_signed_in(m: &mut Model, key: &str) -> Out {
    let out = act(m, key, Act::RestartAgent("continue".into()));
    m.flash("carrying on as the login in use");
    out
}
