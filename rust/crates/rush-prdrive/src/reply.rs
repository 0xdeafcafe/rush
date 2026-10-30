//! Replying on a review thread, refused when it would start or carry on
//! an argument; and resolving threads.
//! Ported from internal/prdrive/reply.go.

use rush_watch::go::quote;

use crate::github::{Thread, fetch, gql, me_now};
use crate::trust::{Trust, known_bot, reviewer_of};

/// Most a reply may say.
const MAX_REPLY: usize = 700;
/// How often we may speak in one thread: past that, the point goes to the
/// user, not a third reply.
const MAX_REPLIES: usize = 2;

/// Posts body in thread of pull request n, unless it would start or carry
/// on an argument: see why in the error.
pub fn reply(repo: &str, n: i64, thread: &str, body: &str, trust: &Trust) -> Result<(), String> {
    let raw = fetch(repo, n)?;
    let me = me_now()?;
    let us = [me, raw.author.clone()];
    let Some(t) = raw.threads.iter().find(|t| t.id == thread) else {
        return Err(format!("thread {thread} isn't on #{n}"));
    };
    let body = body.trim();
    may_reply(t, body, &us, trust)?;
    // A JSON string is a GraphQL string.
    let text = rush_state::json::to_vec(body).map_err(|e| e.to_string())?;
    let q = format!(
        "mutation{{addPullRequestReviewThreadReply(input:{{pullRequestReviewThreadId:{},body:{}}}){{comment{{url}}}}}}",
        quote(thread),
        String::from_utf8_lossy(&text)
    );
    gql::<serde::de::IgnoredAny>(&q).map(|_| ())
}

/// Whether we may say body in t, us being our logins; why not, when not.
pub fn may_reply(t: &Thread, body: &str, us: &[String], trust: &Trust) -> Result<(), String> {
    let Some(root) = t.comments.first() else {
        return Err("the thread is empty".into());
    };
    let finding = reviewer_of(&root.author, &root.body);
    if finding.is_none() && !us.contains(&root.author.login) && !trust.person(&root.author) {
        return Err(
            "refused: an untrusted author opened this thread; that's the user's to answer".into(),
        );
    }
    if let Some((r, f)) = &finding
        && r.ignored.contains(&f.id.as_str())
    {
        return Err(format!(
            "refused: the user never wants '{}' answered; resolve it",
            f.id
        ));
    }
    if t.resolved {
        return Err("refused: the thread is resolved".into());
    }
    let (mut ours, mut last_ours) = (0, false);
    for c in &t.comments {
        let mine = us.contains(&c.author.login);
        if mine {
            ours += 1;
        }
        if mine || known_bot(&c.author) || trust.person(&c.author) {
            last_ours = mine;
        }
    }
    let runes = body.chars().count();
    if last_ours {
        Err("refused: the last word here is already ours; resolve it, or wait for an answer".into())
    } else if ours >= MAX_REPLIES {
        Err("refused: we've replied here twice; put the point to the user instead".into())
    } else if body.is_empty() {
        Err("refused: the reply is empty".into())
    } else if runes > MAX_REPLY {
        Err(format!(
            "refused: {runes} characters; a reply is a sentence or two of evidence (at most {MAX_REPLY})"
        ))
    } else if body.contains('—') {
        Err("refused: no em dashes".into())
    } else {
        Ok(())
    }
}

/// Resolves review threads.
pub fn resolve(threads: &[String]) -> Result<(), String> {
    for t in threads {
        gql::<serde::de::IgnoredAny>(&format!(
            "mutation{{resolveReviewThread(input:{{threadId:{}}}){{thread{{isResolved}}}}}}",
            quote(t)
        ))
        .map_err(|e| format!("{t}: {e}"))?;
    }
    Ok(())
}
