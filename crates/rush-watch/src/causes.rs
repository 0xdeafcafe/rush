//! The lines of a failed Actions job that say why, picked by linetag's
//! hand-weighted net instead of keywords. New in the port (no Go
//! counterpart); off unless config.json's linetagPicks is on.
//!
//! The net reads every line of the job, scores how likely each is to be
//! the cause, and shows the likely causes of each failed step with the
//! couple of lines after each; when it recognises nothing in a step it
//! shows what log_errors would as well. Same shape as log_errors: a
//! `step: <name>` line before each failed step's lines, at most three
//! steps.

use std::sync::LazyLock;

use linetag::log::parse;
use linetag::model::{Tagger, pick_causes};

use crate::go::clip;
use crate::pr::log_errors;

static TAGGER: LazyLock<Tagger> = LazyLock::new(Tagger::new);

/// At most about n lines of an Actions job's log that say what went
/// wrong, as the net picks them.
pub fn log_causes(log: &str, n: usize) -> Vec<String> {
    let parsed = parse(log);
    let tags = TAGGER.tag(&parsed);
    let picked = pick_causes(&parsed, &tags, n);
    let failed = parsed.failed_steps().count();
    let mut out = Vec::new();
    let mut step = None;
    for i in picked {
        let s = parsed.lines[i].step as usize;
        if step != Some(s) {
            step = Some(s);
            let name = &parsed.steps[s].name;
            if !name.is_empty() {
                out.push(format!("step: {}", clip(name, 80)));
            }
        }
        out.push(parsed.text(i).to_string());
    }
    if failed > 3 {
        out.push(format!("… and {} more failed steps", failed - 3));
    }
    out
}

/// The lines to show for a failed job: the net's pick when linetagPicks
/// is on, else log_errors'.
pub fn log_lines(log: &str, n: usize) -> Vec<String> {
    if rush_state::Store::load().config.linetag_picks {
        log_causes(log, n)
    } else {
        log_errors(log, n)
    }
}
