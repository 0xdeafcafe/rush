//! A row's pull request, found by its branch through rush-gh, else the
//! first its agent linked. Port of internal/fleet/pr.go.

use rush_core::adapter::{Checks, Pr};
use rush_core::agent::{Agent, PrStatus};
use rush_gh::ghpr::{Gh, Status};

use crate::Predicates;

/// GitHub's word on a pull request, as a row keeps it.
pub fn from_status(s: Status) -> PrStatus {
    PrStatus {
        number: s.number,
        title: s.title,
        url: s.url,
        branch: s.branch,
        state: s.state,
        checks: s.checks,
        failing: s.failing,
        review: s.review,
        head_oid: s.head_oid,
        merged_at: s.merged_at,
    }
}

/// Sets a.pr from GitHub's pull requests for its repository, as last said
/// (never waiting on GitHub), in place of its agent's links where both
/// have one; its agent's stay where gh can't say.
pub fn find_pr(a: &mut Agent, gh: Option<&Gh>) {
    a.pr = None;
    let b = a.pr_branch().to_string();
    if !b.is_empty()
        && !a.root.is_empty()
        && let Some(p) = gh.and_then(|g| g.for_branch(&a.root, &b))
    {
        let p = from_status(p);
        a.prs = vec![as_linked(&p)];
        a.pr = Some(p);
        return;
    }
    if let Some(l) = a.prs.first() {
        a.pr = Some(from_linked(l));
    }
}

/// A pull request as the list's older links have it.
pub fn as_linked(p: &PrStatus) -> Pr {
    let mut l = Pr {
        url: p.url.clone(),
        title: p.title.clone(),
        number: p.number as i64,
        state: p.state.to_uppercase(),
        ..Pr::default()
    };
    l.review = match p.review.as_str() {
        "approved" => "APPROVED",
        "changes" => "CHANGES_REQUESTED",
        _ => "",
    }
    .into();
    match p.checks.as_str() {
        "pass" => l.checks.passed = 1,
        "fail" => l.checks.failed = 1,
        "pending" => l.checks.pending = 1,
        _ => {}
    }
    l
}

/// A pull request an agent linked to a session, as a row's.
pub fn from_linked(l: &Pr) -> PrStatus {
    let Checks {
        passed,
        failed,
        pending,
    } = l.checks;
    PrStatus {
        number: l.number.max(0) as u64,
        title: l.title.clone(),
        url: l.url.clone(),
        state: l.state.to_lowercase(),
        review: match l.review.to_uppercase().as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes",
            _ => "",
        }
        .into(),
        checks: if failed > 0 {
            "fail"
        } else if pending > 0 {
            "pending"
        } else if passed > 0 {
            "pass"
        } else {
            ""
        }
        .into(),
        ..PrStatus::default()
    }
}
