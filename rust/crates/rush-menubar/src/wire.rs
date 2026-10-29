//! What the feed and the app say to each other, one JSON object a line:
//! the menu's state from the feed, ops from the app. Port of
//! internal/menubar/feed.go:32-130 (State, Account, Window, Working,
//! Waiting, Op), written as Go's jsonx writes them so either rush's app
//! reads either rush's feed.

use rush_core::time::Ms;
use rush_core::wire;
use serde::{Deserialize, Serialize};

/// How many working agents the menu lists.
pub const TOP: usize = 5;

/// Everything the menu shows, sent whenever it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    #[serde(deserialize_with = "wire::nullvec::deserialize")]
    pub accounts: Vec<Account>,
    #[serde(deserialize_with = "wire::nullvec::deserialize")]
    pub working: Vec<Working>,
    /// Working agents past TOP.
    pub more: i64,
    #[serde(deserialize_with = "wire::nullvec::deserialize")]
    pub waiting: Vec<Waiting>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan: String,
    #[serde(rename = "fiveHour")]
    pub five_hour: Window,
    #[serde(rename = "sevenDay")]
    pub seven_day: Window,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub problem: String,
    pub live: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub current: bool,
}

/// One of an account's limits. The feed has room for two: the first two of
/// the account's that limit every model, which for Claude are the 5-hour
/// and weekly ones, and fiveHour and sevenDay are their names in it for the
/// app's sake. Label and Name say which they really are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub present: bool,
    #[serde(with = "wire::float")]
    pub percent: f64,
    #[serde(
        rename = "resetsAt",
        with = "wire::time",
        skip_serializing_if = "is_zero_time"
    )]
    pub resets_at: Ms,
    /// "5h", "7d"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// "5-hour", "weekly"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Working {
    pub key: String,
    pub name: String,
    pub doing: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub repo: String,
    pub account: String,
    #[serde(with = "wire::time")]
    pub since: Ms,
}

/// An agent waiting on you. Kind says how it can be answered from outside
/// rush: "question" (pick an option or type an answer), "permission"
/// (allow or deny), "limit" (continue at the reset or not), or "" (only in
/// rush or its terminal).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Waiting {
    pub key: String,
    pub name: String,
    pub needs: String,
    pub account: String,
    #[serde(skip_serializing_if = "is_false")]
    pub seen: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The request an answer is for.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub req: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub header: String,
    /// The question, whole.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", with = "wire::nullvec")]
    pub options: Vec<String>,
    #[serde(with = "wire::time")]
    pub since: Ms,
}

/// Something the app asks the feed to do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Op {
    /// answer, allow, deny, limit, show
    pub op: String,
    pub key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub req: String,
    /// An option's label or your own words.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub answer: String,
    /// limit: continue at the reset.
    #[serde(skip_serializing_if = "is_false")]
    pub yes: bool,
}

/// One line from the feed as the app reads it: the menu's state, or an
/// error to tell.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Line {
    pub accounts: Option<Vec<Account>>,
    pub working: Option<Vec<Working>>,
    pub more: Option<i64>,
    pub waiting: Option<Vec<Waiting>>,
    pub error: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero_time(t: &Ms) -> bool {
    *t == 0
}

/// The line that tells the app of an error.
pub fn error_line(e: &str) -> String {
    serde_json::json!({ "error": e }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The feed's lines read as Go's: zero times written as Go's zero, the
    /// rest left out where Go's omitempty and omitzero leave them out.
    #[test]
    fn lines_as_go_writes_them() {
        let s = State {
            accounts: vec![Account {
                name: "work".into(),
                five_hour: Window {
                    present: true,
                    percent: 40.0,
                    label: "5h".into(),
                    ..Window::default()
                },
                live: 2,
                current: true,
                ..Account::default()
            }],
            working: vec![],
            more: 0,
            waiting: vec![Waiting {
                key: "k".into(),
                name: "db".into(),
                needs: "asks".into(),
                options: vec!["A".into()],
                since: 1_700_000_000_500,
                ..Waiting::default()
            }],
        };
        let got = serde_json::to_string(&s).unwrap();
        let want = concat!(
            r#"{"accounts":[{"name":"work","fiveHour":{"present":true,"percent":40,"label":"5h"},"#,
            r#""sevenDay":{"present":false,"percent":0},"live":2,"current":true}],"working":[],"more":0,"#,
            r#""waiting":[{"key":"k","name":"db","needs":"asks","account":"","options":["A"],"#,
            r#""since":"2023-11-14T22:13:20.5Z"}]}"#
        );
        assert_eq!(got, want);
        let back: State = serde_json::from_str(&got).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn ops_as_the_app_writes_them() {
        for (line, want) in [
            (
                r#"{"op":"answer","key":"k","req":"r","answer":"SQLite"}"#,
                Op {
                    op: "answer".into(),
                    key: "k".into(),
                    req: "r".into(),
                    answer: "SQLite".into(),
                    yes: false,
                },
            ),
            (
                r#"{"key":"k","op":"limit","yes":true}"#,
                Op {
                    op: "limit".into(),
                    key: "k".into(),
                    yes: true,
                    ..Op::default()
                },
            ),
            (
                r#"{"op":"show","key":""}"#,
                Op {
                    op: "show".into(),
                    ..Op::default()
                },
            ),
        ] {
            assert_eq!(serde_json::from_str::<Op>(line).unwrap(), want, "{line}");
        }
        assert_eq!(error_line("x \"y\""), r#"{"error":"x \"y\""}"#);
    }

    #[test]
    fn go_times_read_back() {
        let l: Line = serde_json::from_str(
            r#"{"accounts":[],"working":[{"key":"k","name":"n","doing":"d","account":"a","since":"2026-10-29T10:00:00.123456789+01:00"}],"more":3,"waiting":null}"#,
        )
        .unwrap();
        let w = &l.working.as_ref().unwrap()[0];
        assert_eq!(
            w.since,
            rush_core::time::parse_rfc3339("2026-10-29T09:00:00.123Z")
        );
        assert_eq!(l.more, Some(3));
        assert!(l.error.is_none());
    }
}
