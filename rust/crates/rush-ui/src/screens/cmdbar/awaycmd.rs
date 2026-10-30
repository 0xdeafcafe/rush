//! #away, #loop and #back on the open session: its host tells it to keep
//! going at each check-in until the time's up, and saves what it asks
//! for when you're back; #loop is the same with you there; #back, or
//! #away off, says what happened. Port of internal/ui/away.go
//! (awayCommand :19, the back sheet's keys :160, setAway :186); the words
//! and the back sheet's body are list_extras::away's.

use rush_core::text::Line;

use crate::frame::sheet::Sheet;
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::screens::list_extras::away::{Away, back_lines, clock, held_reply, parse_away};
use crate::screens::session_core::host::Op;
use crate::screens::session_core::{agent, conn, open_key, switch};
use crate::style::dur;

/// The protocol a host speaks from which it keeps an away (Info.Proto).
const AWAY_PROTO: i64 = 11;

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// #away, #loop or #back (name) with arg on the open session
/// (awayCommand).
pub fn command(m: &mut Model, name: &str, arg: &str) -> Out {
    let Some(key) = open_key(m) else {
        m.flash_err(&format!("#{name} works in an open rush session"));
        return Out::Draw;
    };
    let c = conn(m, &key);
    if !c.hosted() && c.sleeping {
        let Some(a) = agent(m, &key).cloned() else {
            return Out::Draw;
        };
        let (name, arg) = (name.to_string(), arg.to_string());
        return switch::wake_then(m, &a, move |m| command(m, &name, &arg));
    }
    if !c.hosted() {
        m.flash_err(&format!(
            "#{name} works in rush-mode sessions · /rush moves this one over"
        ));
        return Out::Draw;
    }
    if c.info.proto < AWAY_PROTO {
        m.flash_err(&format!(
            "this session's host is older than this rush · /restart it for #{name}"
        ));
        return Out::Draw;
    }
    let was = c.info.away.as_deref().cloned();
    let looping = name == "loop";
    let arg = arg.trim();
    if name == "back"
        || arg == "off"
        || arg.is_empty() && was.as_ref().is_some_and(|w| w.looping == looping)
    {
        match was {
            None => m.flash("not away or looping"),
            Some(w) if w.looping && name == "back" => {
                m.flash("looping, not away · #loop off ends it")
            }
            Some(w) if !w.looping => open_back(m, &key),
            Some(w) => {
                let text = format!("loop off · {} check-in{} sent", w.nudges, plural(w.nudges));
                send(m, &key, None, text);
            }
        }
        return Out::Draw;
    }
    let mut a = match parse_away(arg, looping, m.now) {
        Ok(a) => a,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    if let Some(w) = was {
        a.held = w.held; // what it saved for you stays till you've seen it
    }
    let text = if looping {
        format!(
            "looping · told to keep going every {} it's idle, till it's all done",
            dur(a.every)
        )
    } else {
        format!(
            "away until {} · told to keep going every {}, what it asks saved for you · rush can close; leave the lid open",
            clock(a.until),
            dur(a.every)
        )
    };
    send(m, &key, Some(a), text);
    Out::Draw
}

/// Sets session key's away off the UI thread, saying text once it's set
/// (cmdErr with SetAway).
fn send(m: &mut Model, key: &str, a: Option<Away>, text: String) {
    let Some(link) = conn(m, key).link.clone() else {
        return;
    };
    let op = Op::Away(a.map(Box::new));
    m.later(
        move || link.op(op),
        move |m, r| {
            match r {
                Ok(()) => m.flash(&text),
                Err(e) => m.fail(&e),
            }
            Out::Draw
        },
    );
}

/// What happened while you were away, on session of: enter ends the away,
/// l goes on in a loop, esc leaves it as it is (backSheet).
pub struct BackSheet {
    of: String,
}

/// Opens the back sheet on session key (openBack).
pub fn open_back(m: &mut Model, key: &str) {
    conn(m, key).back_shown = true;
    m.open_sheet(Box::new(BackSheet {
        of: key.to_string(),
    }));
}

/// Opens the back sheet on an away that ended while you were gone, the
/// first time you're in its session since: once a connection, and not
/// over another sheet (backFromAway, away.go:111).
pub fn back_from_away(m: &mut Model, key: &str) {
    if open_key(m).as_deref() != Some(key) || m.sheet.is_some() {
        return;
    }
    let c = conn(m, key);
    if c.back_shown || !c.info.away.as_ref().is_some_and(|a| !a.on()) {
        return;
    }
    open_back(m, key);
}

/// Session of's away, while it's the one open.
fn away_of(m: &Model, of: &str) -> Option<Away> {
    m.session.as_ref().filter(|v| v.key == of)?;
    m.kept
        .peek::<crate::screens::session_core::Core>()
        .and_then(|c| c.conns.get(of))
        .and_then(|c| c.info.away.as_deref().cloned())
}

impl Sheet for BackSheet {
    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let a = away_of(m, &self.of);
        let detail = m
            .agents
            .iter()
            .find(|a| a.key == self.of)
            .map_or("", |a| a.job.detail.as_str());
        back_lines(a.as_ref(), detail, m.now, w, h)
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => Out::Close,
            s @ ("enter" | "l") => {
                let Some(a) = away_of(m, &self.of) else {
                    return Out::Close;
                };
                let r = held_reply(&a.held);
                if !r.is_empty() {
                    let c = conn(m, &self.of);
                    let r = if c.input.is_empty() {
                        r
                    } else {
                        format!("\n\n{r}")
                    };
                    let mut t = c.input.text.clone();
                    t.extend(r.chars());
                    c.input.pos = t.len();
                    c.input.text = t;
                    c.input.anchor = None;
                }
                let next = (s == "l").then(|| Away {
                    from: m.now,
                    every: a.every,
                    looping: true,
                    ..Away::default()
                });
                let key = self.of.clone();
                set_away(m, &key, next, a.nudges);
                Out::Close
            }
            _ => Out::Idle,
        }
    }
}

/// Ends an away for good, or turns it into a loop (next), waking a host
/// that has gone to sleep since (setAway).
fn set_away(m: &mut Model, key: &str, next: Option<Away>, nudges: usize) {
    let c = conn(m, key);
    if !c.hosted() && c.sleeping {
        if let Some(a) = agent(m, key).cloned() {
            let key = key.to_string();
            switch::wake_then(m, &a, move |m| {
                set_away(m, &key, next, nudges);
                Out::Draw
            });
        }
        return;
    }
    if !c.hosted() {
        return;
    }
    let mut text = format!("welcome back · {nudges} check-in{} sent", plural(nudges));
    if let Some(n) = &next {
        text += &format!(" · looping every {}", dur(n.every));
    }
    send(m, key, next, text);
}
