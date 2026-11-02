//! neighbours is the smallest useful rush plugin: one tool that tells
//! Claude which other agents are working in the same repository, and on
//! which branch, so it doesn't trip over them. It needs only the `list`
//! capability, and shows the whole shape of a plugin: answer `initialize`,
//! offer tools, answer tool calls, call rush back.
//!
//! ```sh
//! cargo build --release -p neighbours
//! mkdir -p ~/.config/rush/plugins/neighbours
//! cp target/release/neighbours plugins/examples/neighbours/plugin.json ~/.config/rush/plugins/neighbours/
//! rush plugin approve neighbours
//! ```
//!
//! Ported from plugins/examples/neighbours/main.go.

use rush_plugin_sdk::proto::{Session, ToolCall};
use rush_plugin_sdk::{Conn, Error, params, tool};
use serde_json::{Value, json};

fn main() {
    rush_plugin_sdk::serve(handle)
}

fn handle(conn: &Conn, method: &str, p: Value) -> Result<Value, Error> {
    match method {
        "initialize" => Ok(json!({})),
        "tools.list" => Ok(json!({"tools": [tool::tool(
            "neighbours",
            "List the other agents working in this repository right now: their branch, state and what they're doing. Check before touching shared files, switching branches or running migrations.",
            tool::schema(&[], &[]),
        )]})),
        // Who's calling comes with the call: rush's id for the session, and
        // its folder.
        "tools.call" => {
            let call: ToolCall = params(p)?;
            let all = conn.call::<_, Vec<Session>>("sessions.list", &());
            Ok(tool::result(
                all.map(|all| neighbours(&all, &call.session, &call.cwd)),
            ))
        }
        _ => Err(Error::no_method(method)),
    }
}

/// The agents other than me working in the repository I'm in, in words.
fn neighbours(all: &[Session], me: &str, cwd: &str) -> String {
    // The main checkout and its worktrees are one repository: compare where
    // they keep their worktrees from, the part before .claude/worktrees.
    let repo = all
        .iter()
        .filter(|s| s.id == me)
        .map(|s| home(&s.repo))
        .next_back()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| home(cwd));
    let mut b = String::new();
    for s in all {
        if s.id == me || s.state == "stopped" || repo.is_empty() || home(&s.repo) != repo {
            continue;
        }
        let branch = if s.branch.is_empty() { "?" } else { &s.branch };
        b += &format!("- {} ({}) on {branch}, {}", s.name, s.id, s.state);
        if s.worktree {
            b += " in its own worktree";
        }
        if !s.detail.is_empty() {
            b += &format!(": {}", s.detail);
        }
        b.push('\n');
    }
    if b.is_empty() {
        return "No other agents are working in this repository.".into();
    }
    b
}

/// The main checkout a checkout belongs to, going by where rush and Claude
/// Code put worktrees.
fn home(repo: &str) -> &str {
    repo.find("/.claude/worktrees/")
        .map_or(repo, |i| &repo[..i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn session(id: &str, name: &str, repo: &str, state: &str) -> Session {
        Session {
            id: id.into(),
            name: name.into(),
            repo: repo.into(),
            state: state.into(),
            ..Default::default()
        }
    }

    #[test]
    fn lists_the_others_in_my_repository() {
        let mut other = session("x", "Other", "/r/.claude/worktrees/w", "working");
        other.branch = "feat".into();
        other.worktree = true;
        other.detail = "testing".into();
        let all = [
            session("me", "Me", "/r", "working"),
            other,
            session("y", "Gone", "/r", "stopped"),
            session("z", "Elsewhere", "/s", "working"),
            session("q", "No branch", "/r", "idle"),
        ];
        assert_eq!(
            neighbours(&all, "me", "/r"),
            "- Other (x) on feat, working in its own worktree: testing\n- No branch (q) on ?, idle\n"
        );
        // Not in the list yet: its folder says where it is.
        assert_eq!(
            neighbours(&all[3..4], "new", "/s/.claude/worktrees/a"),
            "- Elsewhere (z) on ?, working\n"
        );
        assert_eq!(
            neighbours(&all[..1], "me", "/r"),
            "No other agents are working in this repository."
        );
    }

    #[test]
    fn manifest_is_valid() {
        let m = rush_plugin_sdk::Manifest::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(m.command, ["neighbours"]);
        assert_eq!(m.sessions, ["list"]);
    }
}
