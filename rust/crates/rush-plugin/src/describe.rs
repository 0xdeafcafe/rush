//! What approving a plugin allows, in plain words.
//! Ported from internal/plugin/describe.go.

use std::fmt::Write;

use crate::manifest::{
    CAP_CONTROL, CAP_LIST, CAP_QUEUE, CAP_QUEUED, CAP_READ, CAP_SEND, CAP_START, PROTO_MCP,
};
use crate::plugin::Plugin;
use crate::text::{clip, printable};
use crate::ui::{
    INTERCEPT_BUDGET, MAX_PLUGIN_VIEWS, UI_EVENTS, UI_INPUT, UI_INTERCEPT, UI_NOTIFY, UI_OVERVIEW,
    UI_SEND, UI_VIEWS,
};

/// Says, in plain words, what approving p allows: the same text `rush
/// plugin check|approve` shows at a terminal, and what a UI's own approval
/// dialog shows instead of one.
pub fn describe(p: &Plugin) -> String {
    let mut b = String::new();
    b.push_str(&p.name);
    if !p.version.is_empty() {
        let _ = write!(b, " {}", p.version);
    }
    b.push('\n');
    if !p.description.is_empty() {
        let _ = writeln!(b, "  {}", printable(&p.description));
    }
    if !p.category.is_empty() {
        let _ = writeln!(b, "  Listed under {}.", p.category_of().title);
    }
    let _ = writeln!(b, "  {}.", p.from());
    let _ = write!(b, "\nIt runs {}", p.command.join(" "));
    if p.proto() == PROTO_MCP {
        b.push_str(" as an MCP server");
    }
    b.push_str(", sandboxed:\n");
    let _ = writeln!(
        b,
        "  • reads its folder, {}, and the system's libraries",
        p.dir
    );
    for r in &p.read {
        let _ = writeln!(b, "  • also reads {r}");
    }
    let data = crate::data_dir(&p.name);
    if p.write.is_empty() {
        let _ = writeln!(b, "  • writes only {}", data.display());
    } else {
        let _ = writeln!(
            b,
            "  • writes {}, and {}",
            data.display(),
            p.write.join(", ")
        );
    }
    if p.network.is_empty() {
        b.push_str("  • no network\n");
    } else {
        let _ = writeln!(b, "  • reaches only {}", p.network.join(", "));
    }
    let _ = writeln!(
        b,
        "  • starts no other programs itself, at most {} MB of memory",
        p.memory() >> 20
    );
    if !p.exec.is_empty() {
        b.push_str(
            "\nOutside the sandbox, as you, rush runs for it (with any arguments it adds):\n",
        );
        for argv in p.exec.values() {
            let _ = writeln!(b, "  • {}", argv.join(" "));
        }
    }

    let mut grants: Vec<String> = Vec::new();
    let ws = p.workspaces.join(", ");
    if p.has_tools() {
        grants.push("offers its tools to every rush-mode session (each call asks you first, as any tool does)".into());
    }
    if !p.agents.is_empty() {
        let names: Vec<String> = p.agents.keys().map(|n| format!("{}:{n}", p.name)).collect();
        grants.push(format!(
            "adds subagents to every rush-mode session: {}",
            names.join(", ")
        ));
    }
    if !p.prompt.is_empty() {
        grants.push(format!(
            "adds {} characters to every rush-mode session's system prompt:\n      {}",
            p.prompt.len(),
            clip(&p.prompt, 400).replace('\n', "\n      ")
        ));
    }
    if p.can(CAP_LIST) {
        grants.push("sees every rush-mode session's name, folder, branch, state, cost and context use (not what was said)".into());
    }
    if p.can(CAP_START) {
        grants.push(format!(
            "starts sessions on your account in {ws}, never in a mode that skips asking you;\n      tools your Claude Code settings already allow run there without asking"
        ));
    }
    if p.can(CAP_READ) {
        grants.push("follows what the sessions it started say".into());
    }
    if p.can(CAP_SEND) {
        grants.push("sends messages to the sessions it started".into());
    }
    if p.can(CAP_QUEUE) {
        grants.push(format!(
            "queues messages, marked as its own, to any session in {ws} that asks you before acting"
        ));
    }
    if p.can(CAP_CONTROL) {
        grants.push("interrupts and stops the sessions it started".into());
    }
    if p.can(CAP_QUEUED) {
        grants.push(format!("sends now, or drops, the messages you queued in any session in {ws}, by their place in the queue, without seeing them"));
    }
    if p.sidebar {
        grants.push("arranges your agent list: groups and names agents in sections of its own, offered as a group-by mode".into());
    }
    if p.can_ui(UI_INPUT) {
        grants.push("SEES EVERYTHING YOU TYPE in rush's message boxes, as you type it and when you send or clear it, and may set what's in them".into());
    }
    if p.can_ui(UI_INTERCEPT) {
        grants.push(format!(
            "is asked before each message you send goes, and may change it or hold it back (it has {}ms; after that it goes as it was)",
            INTERCEPT_BUDGET.as_millis()
        ));
    }
    if p.can_ui(UI_EVENTS) {
        grants.push("hears what happens in rush's screen: sessions seen, opened and left, turns starting and ending, why a session stopped, the network going and coming back; with each, what the agent list shows of the session (never what was said)".into());
    }
    if p.can_ui(UI_SEND) {
        grants.push(format!(
            "sends messages, as if you'd typed them, to sessions in {ws} that ask you before acting"
        ));
    }
    if p.can_ui(UI_OVERVIEW) {
        grants.push(
            "adds sections to a Session's overview, and a word or two to its row in the list"
                .into(),
        );
    }
    if p.can_ui(UI_VIEWS) {
        grants.push(format!(
            "draws up to {MAX_PLUGIN_VIEWS} views of its own beside a session's conversation, named as its own"
        ));
    }
    if p.can_ui(UI_NOTIFY) {
        grants.push("shows short notices at the bottom of rush's screen".into());
    }
    if !p.commands.is_empty() {
        let cs: Vec<String> = p
            .commands
            .iter()
            .map(|c| {
                if c.key.is_empty() {
                    c.name.clone()
                } else {
                    format!("{} ({}, if it's free)", c.name, c.key)
                }
            })
            .collect();
        grants.push(format!(
            "adds commands you can run and bind to keys: {}",
            cs.join(", ")
        ));
    }
    if !p.cli.is_empty() {
        let cs: Vec<String> = p
            .cli
            .iter()
            .map(|c| format!("rush {} {}", p.name, c.name))
            .collect();
        grants.push(format!(
            "adds commands to rush's CLI, run by the plugin with what it may do: {}",
            cs.join(", ")
        ));
    }
    if !p.settings.is_empty() {
        let ss: Vec<&str> = p.settings.iter().map(|s| s.title.as_str()).collect();
        grants.push(format!(
            "offers settings under Settings, Plugins: {}",
            ss.join(", ")
        ));
    }
    if !p.provides.is_empty() {
        grants.push(format!(
            "answers other plugins and rush when they ask for {}",
            p.provides.join(", ")
        ));
    }
    if !p.uses.is_empty() {
        grants.push(format!(
            "asks the plugins that provide {} for what they know",
            p.uses.join(", ")
        ));
    }
    let needs = p.requires.needs();
    if !needs.is_empty() {
        let _ = writeln!(b, "\nIt runs only with {needs}.");
    }
    if !grants.is_empty() {
        b.push_str("\nIt:\n");
        for g in grants {
            let _ = writeln!(b, "  • {g}");
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CAP_LIST, Manifest};
    use crate::plugin::tests::Home;

    #[test]
    fn describes_what_approving_allows() {
        let _h = Home::new();
        let p = Plugin {
            manifest: Manifest {
                name: "neighbours".into(),
                version: "1".into(),
                description: "Tells \u{1b}[31mClaude".into(),
                category: "agents".into(),
                command: vec!["neighbours".into()],
                tools: true,
                sessions: vec![CAP_LIST.into()],
                memory_mb: 32,
                ui: vec![UI_INTERCEPT.into(), UI_INPUT.into()],
                ..Default::default()
            },
            dir: "/p/neighbours".into(),
            ..Default::default()
        };
        let got = describe(&p);
        let data = crate::data_dir("neighbours");
        let want = format!(
            "neighbours 1\n  Tells [31mClaude\n  Listed under Agents.\n  Put in /p/neighbours by hand: rush doesn't know where it came from.\n\n\
It runs neighbours, sandboxed:\n  • reads its folder, /p/neighbours, and the system's libraries\n  • writes only {}\n  • no network\n  \
• starts no other programs itself, at most 32 MB of memory\n\nIt:\n  \
• offers its tools to every rush-mode session (each call asks you first, as any tool does)\n  \
• sees every rush-mode session's name, folder, branch, state, cost and context use (not what was said)\n  \
• SEES EVERYTHING YOU TYPE in rush's message boxes, as you type it and when you send or clear it, and may set what's in them\n  \
• is asked before each message you send goes, and may change it or hold it back (it has 400ms; after that it goes as it was)\n",
            data.display()
        );
        assert_eq!(got, want);
    }
}
