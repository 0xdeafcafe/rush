//! Sorts the error a turn stopped on by what it says about the provider,
//! and says it briefly without anything the error carried that could be a
//! secret: what health keeps.
//!
//! Every harness words its errors its own way, so the sorting is by
//! phrases: Claude Code's "API Error: 401 …", Codex's "unexpected status
//! 429 Too Many Requests", an OAuth server's invalid_grant. An adapter
//! whose agent says more adds its phrases with add.
//!
//! Ported from internal/agent/failure/failure.go.

use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};

/// What an error says about the provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An error trying again doesn't get past and that says nothing about
    /// the provider: the prompt too long, a bad request, anything not known.
    Fatal,
    /// A usage limit: the account's plan has run out until it resets.
    Limit,
    /// A login that no longer works: signed out, the token expired or
    /// revoked, the key refused.
    Auth,
    /// The provider busy or rate limiting: never the login, and trying
    /// again later gets past it.
    Capacity,
    /// The network down, or the machine asleep: not the provider.
    Offline,
    /// The provider failing for a moment: a dropped or stalled connection,
    /// a 5xx, a timeout.
    Transient,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Fatal => "fatal",
            Kind::Limit => "limit",
            Kind::Auth => "auth",
            Kind::Capacity => "capacity",
            Kind::Offline => "offline",
            Kind::Transient => "transient",
        }
    }

    /// Whether trying again gets past an error of this kind.
    pub fn retries(self) -> bool {
        matches!(self, Kind::Capacity | Kind::Offline | Kind::Transient)
    }
}

struct Tables {
    /// What each kind's errors say, lowercased, looked for in this order: a
    /// usage limit first, as its words ("limit reached") come with
    /// rate-limit ones too. Codes of three digits match only as a number
    /// of their own, so "4013 tokens" isn't a 401.
    phrases: Vec<(Kind, Vec<String>)>,
    /// The only words a Summary may hold: error codes of providers and
    /// OAuth, network errors and a few plain words. A fixed list, so
    /// nothing from a token or key can ever pass, whatever its shape.
    known: HashSet<String>,
}

static TABLES: LazyLock<RwLock<Tables>> = LazyLock::new(|| {
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let phrases = vec![
        (
            Kind::Limit,
            v(&["usage limit", "limit reached", "usage_limit_reached"]),
        ),
        (
            Kind::Auth,
            v(&[
                "401",
                "403",
                "invalid_grant",
                "invalid_token",
                "invalid_client",
                "invalid_api_key",
                "invalid api key",
                "invalid x-api-key",
                "unauthorized_client",
                "access_denied",
                "authentication",
                "permission_error",
                "unauthorized",
                "forbidden",
                "revoked",
                "oauth token",
                "failed to authenticate",
                "oauth session expired",
                "token has expired",
                "token expired",
                "expired token",
                "not logged in",
                "/login",
                "please log in",
                "log in again",
                "apikeyhelper",
            ]),
        ),
        (
            Kind::Capacity,
            v(&[
                "429",
                "529",
                "overloaded",
                "rate_limit",
                "rate limit",
                "too many requests",
                "insufficient_quota",
                "quota",
            ]),
        ),
        (
            Kind::Offline,
            v(&[
                "can't reach the api",
                "unable to connect to api",
                "no response from api",
                "went to sleep",
                "internet",
                "enotfound",
                "eai_again",
                "econnrefused",
                "enetunreach",
                "ehostunreach",
                "enetdown",
            ]),
        ),
        (
            Kind::Transient,
            v(&[
                "500",
                "502",
                "503",
                "504",
                "api error: 5",
                "internal server error",
                "server_error",
                "api_error",
                "temporarily",
                "service unavailable",
                "timed out",
                "timeout",
                "etimedout",
                "econnreset",
                "socket hang up",
                "connection error",
                "connection lost",
                "connection reset",
                "connection closed",
                "mid-response",
                "mid-stream",
                "stopped arriving",
            ]),
        ),
    ];
    let known = [
        "invalid_grant",
        "invalid_token",
        "invalid_client",
        "invalid_request",
        "invalid_api_key",
        "unauthorized_client",
        "access_denied",
        "authentication_error",
        "permission_error",
        "insufficient_quota",
        "rate_limit_error",
        "rate_limit_exceeded",
        "usage_limit_reached",
        "overloaded_error",
        "api_error",
        "server_error",
        "not_found_error",
        "model_not_found",
        "invalid_request_error",
        "context_length_exceeded",
        "request_too_large",
        "unauthorized",
        "forbidden",
        "expired",
        "revoked",
        "overloaded",
        "timeout",
        "aborted",
        "quota",
        "unavailable",
        "econnrefused",
        "econnreset",
        "enotfound",
        "etimedout",
        "eai_again",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    RwLock::new(Tables { phrases, known })
});

/// Has errors that say any of says (any case) sort as k, for an adapter
/// whose agent words its errors its own way. A single word is let into
/// Summary too.
pub fn add(k: Kind, says: &[&str]) {
    let mut t = TABLES.write().unwrap_or_else(|e| e.into_inner());
    let Tables { phrases, known } = &mut *t;
    for (kind, list) in phrases.iter_mut() {
        if *kind != k {
            continue;
        }
        for s in says {
            let s = s.to_lowercase();
            if !s.chars().any(not_word) {
                known.insert(s.clone());
            }
            list.push(s);
        }
    }
}

/// What err says about the provider.
pub fn kind_of(err: &str) -> Kind {
    let t = err.to_lowercase();
    if t.contains("hit your") && t.contains("limit") {
        return Kind::Limit; // "You've hit your session limit · resets 5am"
    }
    let tables = TABLES.read().unwrap_or_else(|e| e.into_inner());
    for (kind, says) in &tables.phrases {
        if says.iter().any(|s| contains(&t, s)) {
            return *kind;
        }
    }
    Kind::Fatal
}

/// Whether t says s; a code of digits only as a number of its own.
fn contains(t: &str, s: &str) -> bool {
    if !is_digits(s) {
        return t.contains(s);
    }
    let tb = t.as_bytes();
    let mut from = 0;
    while let Some(i) = t[from..].find(s) {
        let i = i + from;
        let end = i + s.len();
        if (i == 0 || !tb[i - 1].is_ascii_alphanumeric())
            && (end == tb.len() || !tb[end].is_ascii_alphanumeric())
        {
            return true;
        }
        from = end;
    }
    false
}

/// err said briefly and safely: its HTTP status, when it gives one ("401",
/// "status 429", "HTTP 503", "API Error: 529"), then the known words it
/// holds, in order, once each, five at most. Nothing else of it is kept.
/// With none of that, it is what its kind means in plain words.
pub fn summary(err: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut prev = String::new();
    {
        let tables = TABLES.read().unwrap_or_else(|e| e.into_inner());
        for w in err.split(not_word).filter(|w| !w.is_empty()) {
            let lw = w.to_lowercase();
            if parts.is_empty()
                && is_status(w)
                && (prev.is_empty() || prev == "status" || prev == "http" || prev == "error")
            {
                parts.push(w.to_string());
            } else if tables.known.contains(&lw) && !parts.contains(&lw) {
                parts.push(lw.clone());
            }
            if parts.len() >= 5 {
                break;
            }
            prev = lw;
        }
    }
    if !parts.is_empty() {
        return parts.join(" ");
    }
    match kind_of(err) {
        Kind::Limit => "usage limit",
        Kind::Auth => "signed out",
        Kind::Capacity => "rate limited",
        Kind::Offline => "offline",
        _ => "request failed",
    }
    .into()
}

/// An HTTP status a failure gives: 4xx or 5xx.
fn is_status(w: &str) -> bool {
    w.len() == 3 && (w.starts_with('4') || w.starts_with('5')) && is_digits(w)
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())
}

/// Splits a Summary's words: letters, digits and underscores make them.
fn not_word(r: char) -> bool {
    !(r.is_ascii_alphanumeric() || r == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_summarises() {
        for (err, kind, sum) in [
            (
                "API Error: 401 invalid_grant sk-secret",
                Kind::Auth,
                "401 invalid_grant",
            ),
            (
                "API Error: 429 rate_limit_error",
                Kind::Capacity,
                "429 rate_limit_error",
            ),
            (
                "API Error: Connection error.",
                Kind::Transient,
                "request failed",
            ),
            ("API Error: 500", Kind::Transient, "500"),
            ("Prompt is too long", Kind::Fatal, "request failed"),
            (
                "You've hit your session limit · resets 5am",
                Kind::Limit,
                "usage limit",
            ),
            ("used 4013 tokens", Kind::Fatal, "request failed"),
            (
                "unexpected status 429 Too Many Requests",
                Kind::Capacity,
                "429",
            ),
            (
                "Not logged in · Please run /login",
                Kind::Auth,
                "signed out",
            ),
            ("getaddrinfo ENOTFOUND api", Kind::Offline, "enotfound"),
        ] {
            assert_eq!(kind_of(err), kind, "{err}");
            assert_eq!(summary(err), sum, "{err}");
        }
    }

    // Go's TestKindOf, table for table.
    #[test]
    fn kind_of_as_go() {
        use Kind::*;
        for (err, want) in [
            // Usage limits, as host.isLimit read them.
            ("Claude usage limit reached", Limit),
            ("You've hit your session limit · resets 5am", Limit),
            ("You've hit your weekly limit", Limit),
            ("usage_limit_reached", Limit),
            // Signed out.
            ("Not logged in · Please run /login", Auth),
            (
                r#"API Error: 401 {"type":"error","error":{"type":"authentication_error"}}"#,
                Auth,
            ),
            ("OAuth token has expired. Please obtain a new token", Auth),
            ("Invalid API key · Please run /login", Auth),
            (
                "Failed to authenticate: OAuth session expired and could not be refreshed",
                Auth,
            ),
            ("apiKeyHelper failed", Auth),
            (
                r#"API Error: 403 {"error":{"type":"permission_error"}}"#,
                Auth,
            ),
            ("invalid_grant: refresh token revoked", Auth),
            ("unexpected status 401 Unauthorized", Auth),
            // Busy or rate limited, never the login.
            ("API Error: 529 Overloaded", Capacity),
            (r#"API Error: 429 {"type":"rate_limit_error"}"#, Capacity),
            (
                "exceeded retry limit, last status: 429 Too Many Requests",
                Capacity,
            ),
            ("insufficient_quota", Capacity),
            // The network.
            (
                "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
                Offline,
            ),
            ("API Error: Unable to connect to API (ENOTFOUND)", Offline),
            (
                "API Error: No response from API (waited 5m, then 10m on the retry).",
                Offline,
            ),
            (
                "API Error: Your computer went to sleep mid-response.",
                Offline,
            ),
            // Failing for a moment.
            ("API Error: Connection lost mid-response.", Transient),
            ("API Error: Response stalled mid-stream.", Transient),
            ("API Error: Connection error.", Transient),
            ("API Error: 500 Internal server error", Transient),
            ("API Error: 503 service unavailable", Transient),
            ("Request timed out", Transient),
            // Nothing about the provider, and words too broad to count.
            ("Prompt is too long", Fatal),
            ("error_during_execution", Fatal),
            ("failed to login to the MCP server's dashboard", Fatal),
            ("used 4013 tokens", Fatal),
            ("", Fatal),
        ] {
            assert_eq!(kind_of(err), want, "{err:?}");
        }
    }

    // Go's TestSummary.
    #[test]
    fn summary_as_go() {
        for (err, want) in [
            (
                r#"API Error: 401 {"error":{"type":"authentication_error","message":"sk-ant-oat01-abcdef invalid"}}"#,
                "401 authentication_error",
            ),
            (
                "unexpected status 401 Unauthorized: Bearer eyJhbGciOi",
                "401 unauthorized",
            ),
            (
                "exceeded retry limit, last status: 429 Too Many Requests",
                "429",
            ),
            (
                "invalid_grant invalid_grant revoked expired forbidden unauthorized quota",
                "invalid_grant revoked expired forbidden unauthorized",
            ),
            ("connect ECONNREFUSED 127.0.0.1:443", "econnrefused"),
            ("Not logged in · Please run /login", "signed out"),
            ("Connection lost mid-response.", "request failed"),
            ("my token is hunter2", "request failed"),
            ("the page said 404 twice", "request failed"),
        ] {
            assert_eq!(summary(err), want, "{err:?}");
        }
    }

    // An adapter's word sorts and summarises.
    #[test]
    fn add_words() {
        add(Kind::Auth, &["Refresh_Token_Reused"]);
        assert_eq!(kind_of("error: refresh_token_reused"), Kind::Auth);
        assert_eq!(summary("refresh_token_reused"), "refresh_token_reused");
    }
}
