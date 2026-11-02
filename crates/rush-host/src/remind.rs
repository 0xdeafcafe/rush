//! A session's reminders, as its agent tools set and list them: rush sends
//! the session a message at a time it picks.
//!
//! Ported from internal/host/remind.go:1-53.

use rush_state::Time;

use crate::subtool::SubAgents;

impl SubAgents {
    /// Has rush send the session text at at (agtools' remind).
    pub fn remind(&self, at: &str, text: &str) -> Result<String, String> {
        let now = Time::now();
        let due = rush_remind::parse(at, now)?;
        let r = rush_remind::add(self.parent(), text, due, now)?;
        Ok(format!(
            "Set: reminder {}, due {} ({}). rush will send you its text then; end your turn rather than wait for it.",
            r.id,
            rush_remind::due_words(r.due),
            rush_remind::until(r.due, now)
        ))
    }

    /// Lists the session's reminders, or cancels one first (agtools'
    /// reminders).
    pub fn reminders(&self, cancel: &str) -> Result<String, String> {
        let mut done = String::new();
        match cancel.trim() {
            "" => {}
            "all" => {
                rush_remind::cancel_all(self.parent())?;
                done = "Cancelled them all.\n".into();
            }
            id => {
                rush_remind::cancel(self.parent(), id)?;
                done = format!("Cancelled {id}.\n");
            }
        }
        let now = Time::now();
        let list = rush_remind::list(self.parent());
        if list.is_empty() {
            return Ok(done + "No reminders set.");
        }
        let lines: Vec<String> = list.iter().map(|r| rush_remind::line(r, now)).collect();
        Ok(done + &lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    #[test]
    fn sets_lists_and_cancels() {
        let _h = Home::new();
        rush_remind::no_waiters(true);
        let id = "rm-0000-aaaa";
        std::fs::create_dir_all(crate::paths::dir(id)).unwrap();
        std::fs::write(
            crate::paths::dir(id).join("config.json"),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
        let sa = SubAgents::of(rush_core::adapter::Registry::new(), id).unwrap();
        let said = sa.remind("20m", "check the deploy").unwrap();
        assert!(
            said.starts_with("Set: reminder ") && said.contains("(in 20m)"),
            "{said}"
        );
        let listed = sa.reminders("").unwrap();
        assert!(
            listed.contains("check the deploy") && listed.contains("(in 20m)"),
            "{listed}"
        );
        assert!(
            sa.reminders("nope")
                .unwrap_err()
                .contains("no reminder nope")
        );
        assert_eq!(
            sa.reminders("all").unwrap(),
            "Cancelled them all.\nNo reminders set."
        );
        assert!(
            sa.remind("soon", "x")
                .unwrap_err()
                .contains("can't read \"soon\"")
        );
        rush_remind::no_waiters(false);
    }

    // cmd/rush/remind.go: set, list and cancel by the command.
    #[test]
    fn the_command() {
        let _h = Home::new();
        rush_remind::no_waiters(true);
        let run = |session: &str, args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = crate::cli::remind_cmd_in(session, &args, &mut out, &mut err);
            (
                code,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        };
        let (code, _, err) = run("", &["25m", "x"]);
        assert_eq!(
            (code, err.as_str()),
            (
                1,
                "rush remind: not in a session rush runs; name one with --session <id>\n"
            )
        );
        let (code, out, _) = run("", &["--session", "s1", "25m", "look", "again"]);
        assert!(
            code == 0 && out.starts_with("set ") && out.contains("(in 25m)  look again"),
            "{out}"
        );
        let (_, out, _) = run("s1", &["list"]);
        assert!(out.contains("look again"), "{out}");
        let (_, out, _) = run("", &["list", "--all"]);
        assert!(out.trim_end().ends_with("  · s1"), "{out}");
        assert_eq!(run("s1", &["cancel", "all"]).1, "cancelled\n");
        assert_eq!(run("s1", &["ls"]).1, "no reminders\n");
        assert_eq!(run("s1", &["help"]).1, crate::cli::REMIND_USAGE);
        rush_remind::no_waiters(false);
    }
}
