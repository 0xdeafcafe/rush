//! The / picker over a session's box and rush's own / commands: what the
//! picker offers and in what order, its rows as Go draws them at 80, 120
//! and 200 columns, its keys, a command's values, and the commands rush
//! answers itself, /model and /effort's pick kept and put back when the
//! host refuses it. After Go's internal/ui/question_test.go
//! (TestSlashQueueTasks, TestSlashMidMessage, TestArgPicker,
//! TestSlashFullWordSends, TestClaudeScreensAndFork's picker),
//! sheets_test.go (TestSlashAliasComesFirst), infosheet_test.go's /log
//! and /them, choicesheet_test.go (TestChoicePicker) and
//! sleep_host_test.go's setArg. testdata/go_slash_picker.txt is Go's
//! slashLines for the same session, ANSI stripped.

use std::sync::Arc;

use rush_core::adapter::{
    Adapter, Choices, Chooser, Features, Kind, Level, Profile, Registry,
};
use rush_core::event::Command;
use rush_core::text::Line;

use super::host::{Fake, Hosts, Op, set_hosts};
use super::rushcmd::{self, ArgSheet};
use super::slash::{self, Seen, arg_matches, slash_matches, slash_word};
use super::tests::flash;
use super::tests_subview::KEY;
use super::{conn, keys};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::{BLUE, ORANGE, SUB};

fn cmd(name: &str, description: &str) -> Command {
    Command {
        name: name.into(),
        description: description.into(),
        ..Command::default()
    }
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// The names the picker offers for input, the cursor back from its end,
/// in a Claude Code session that said it has said, with local on disk.
fn names(m: &Model, said: &[Command], local: &[Command], input: &str, back: usize) -> Vec<String> {
    let s = Seen {
        reg: &m.reg,
        kind: "claude",
        said,
        local,
    };
    let input = chars(input);
    slash_matches(&s, &input, input.len() - back)
        .into_iter()
        .map(|c| c.name)
        .collect()
}

/// Claude Code as rush-claude has it (its features Go's, cd, btw and
/// compact included), with no profiles of its own on disk.
struct GoClaude(rush_claude::Claude);

impl Adapter for GoClaude {
    fn kind(&self) -> Kind {
        self.0.kind()
    }
    fn name(&self) -> String {
        self.0.name()
    }
    fn features(&self) -> Features {
        self.0.features()
    }
    fn level(&self) -> Level {
        self.0.level()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(chooser: Chooser);
}

impl Chooser for GoClaude {
    fn choices(&self) -> Choices {
        self.0
            .chooser()
            .expect("Claude Code lists its choices")
            .choices()
    }
}

/// A Claude Code session open, hosted by fake, its commands said and the
/// ones on disk read (so nothing's read from disk).
fn session(d: &tempfile::TempDir) -> (Model, Arc<Fake>) {
    let mut m = super::tests_subview::model(d);
    let reg = Registry::new();
    reg.register(GoClaude(rush_claude::Claude::new()));
    m.reg = reg;
    m.cards().host = Some(super::cards_host::host());
    m.agents[0].job.id = "h1".into();
    m.agents[0].pid = 4242;
    m.agents[0].rush = true;
    let fake = Fake::new();
    set_hosts(&mut m, Arc::new(fake.clone()));
    fake.host(KEY);
    let a = m.agents[0].clone();
    let now = m.now;
    let c = conn(&mut m, KEY);
    c.link = Some(fake.dial(&a).unwrap());
    c.info.model = "claude-sonnet-5".into();
    c.local = vec![
        cmd(
            "release-check",
            "check the release notes and tag before shipping",
        ),
        cmd("review-pr", "review the open pull request, file by file"),
    ];
    c.skills = ["review-pr".to_string()].into();
    c.local_at = Some(now);
    let s = &mut m.session.as_mut().unwrap().sess;
    s.command_list = vec![
        cmd(
            "compact",
            "Clear conversation history but keep a summary in context. Optional: /compact [instructions for summarization]",
        ),
        cmd("review", "Review a pull request"),
        cmd("context", ""),
    ];
    s.commands = s.command_list.iter().map(|c| c.name.clone()).collect();
    crate::screens::settings::use_store(&mut m, rush_state::Store::default());
    (m, fake)
}

fn typed(m: &mut Model, text: &str, back: usize) {
    let c = conn(m, KEY);
    c.input.set(text);
    c.input.pos = c.input.text.len() - back;
}

fn press(m: &mut Model, k: &str) -> Out {
    m.press_key(&Press::of(k).expect("a key"))
}

fn input(m: &mut Model) -> String {
    conn(m, KEY).input.string()
}

/// rush's and Claude Code's screens first, then the session's, then
/// claude: ones only an agent with screens of its own has; rush's /clear,
/// not #clean; nothing once arguments start; rush's own still at the
/// start (TestSlashQueueTasks, TestSlashMidMessage).
#[test]
fn the_picker_offers_rushs_then_the_sessions() {
    let d = tempfile::tempdir().unwrap();
    let (m, _) = session(&d);
    let said = [
        cmd("compact", "summarise"),
        cmd("context", ""),
        cmd("code-review", ""),
    ];
    let got = names(&m, &said, &[], "/co", 0);
    assert!(got.len() >= 5, "{got:?}");
    assert_eq!(
        &got[..4],
        ["copy", "context", "config", "compact"],
        "{got:?}"
    );
    assert!(got.last().unwrap().starts_with("claude:"), "{got:?}");
    assert_eq!(names(&m, &said, &[], "/clea", 0), ["clear"]);
    assert!(names(&m, &said, &[], "/compact now", 0).is_empty());
    assert!(names(&m, &said, &[], "/cl", 0).contains(&"clear".to_string()));
}

/// Mid-message only the session's and the disk's are offered, and enter
/// completes; a path is not a command (TestSlashMidMessage).
#[test]
fn mid_message_completes_and_a_path_is_no_command() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    let local = [cmd("design:design-critique", ""), cmd("pdf", "")];
    assert_eq!(
        names(&m, &[], &local, "please /crit this", 5),
        ["design:design-critique"]
    );
    assert!(names(&m, &[], &local, "see /var/folders/x", 0).is_empty());
    assert_eq!(
        slash_word(&chars("see /var"), 8),
        Some((4, 8, "var".into()))
    );
    conn(&mut m, KEY).local = local.to_vec();
    typed(&mut m, "please /crit this", 5);
    assert_eq!(press(&mut m, "enter"), Out::Draw);
    assert_eq!(input(&mut m), "please /design:design-critique this");
    assert!(fake.ops().is_empty(), "completed, not sent");
}

/// Another name for one of Claude Code's screens finds it first, and a
/// name typed exactly comes first (TestSlashAliasComesFirst); claude:login
/// is found by login, and what's off isn't offered (infosheet_test.go).
#[test]
fn aliases_and_exact_names_first() {
    let d = tempfile::tempdir().unwrap();
    let (m, _) = session(&d);
    let said = [cmd("reload-plugins", ""), cmd("compact", "")];
    let got = names(&m, &said, &[], "/plugins", 0);
    assert!(got.len() >= 2 && got[0] == "plugin", "{got:?}");
    assert_eq!(names(&m, &said, &[], "/compact", 0)[0], "compact");
    assert_eq!(names(&m, &said, &[], "/log", 0)[0], "claude:login");
    assert!(names(&m, &said, &[], "/them", 0).is_empty());
    // Under Claude Code's own name a cloud one isn't offered.
    let got = names(&m, &[cmd("login", "")], &[], "/login", 0);
    assert_eq!(got[0], "claude:login");
    assert!(!got.contains(&"login".to_string()), "{got:?}");
}

/// The picker's rows read as Go's, word for word and space for space, at
/// 80, 120 and 200 columns: rush's commands and screens, the session's,
/// skills, claude: ones, a command's values, mid-message, scrolled.
#[test]
fn the_picker_reads_as_gos() {
    let cases: [(&str, usize, usize); 10] = [
        ("/co", 0, 0),
        ("/co", 0, 3),
        ("/", 0, 0),
        ("/", 0, 9),
        ("/log", 0, 0),
        ("/plugins", 0, 0),
        ("/model ", 0, 0),
        ("/effort x", 0, 0),
        ("please /re this", 5, 0),
        ("/rev", 0, 0),
    ];
    let d = tempfile::tempdir().unwrap();
    let mut out = String::new();
    for w in [80, 120, 200] {
        for (text, back, sel) in cases {
            let (mut m, _) = session(&d);
            typed(&mut m, text, back);
            conn(&mut m, KEY).slash_sel = sel;
            out += &format!("=== {w} {text:?} back={back} sel={sel}\n");
            for l in slash::slash_lines(&mut m, KEY, w) {
                out += &format!("{}|\n", l.plain());
            }
        }
    }
    let want = include_str!("testdata/go_slash_picker.txt");
    for (i, (g, w)) in out.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "line {}", i + 1);
    }
    assert_eq!(out, want);
}

/// The span drawn in colour c, if any.
fn has(l: &Line, text: &str, c: rush_core::text::Color) -> bool {
    l.spans.iter().any(|s| s.text == text && s.style.fg == c)
}

/// rush's own are tagged rush in orange, Claude Code's ↗ in its grey, a
/// skill in blue; mid-message rush's screens aren't (slashLines' tag).
#[test]
fn the_tags_are_coloured_as_gos() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    typed(&mut m, "/co", 0);
    let rows = slash::slash_lines(&mut m, KEY, 120);
    assert!(has(&rows[0], " rush", ORANGE), "{:?}", rows[0]);
    assert!(has(&rows[4], " claude code ↗", SUB), "{:?}", rows[4]);
    typed(&mut m, "please /re this", 5);
    let rows = slash::slash_lines(&mut m, KEY, 120);
    assert!(has(&rows[2], " skill", BLUE), "{:?}", rows[2]);
}

/// "/model " offers every model and the agent's own default, the one it
/// runs marked now; "/effort x" xhigh; none before the space
/// (TestArgPicker). A value picked here is now, whatever runs.
#[test]
fn a_commands_values() {
    let d = tempfile::tempdir().unwrap();
    let (m, _) = session(&d);
    let am = |t: &str, picked: Option<&str>| {
        let i = chars(t);
        arg_matches(&m.reg, "claude", &i, i.len(), picked, "claude-sonnet-5")
    };
    let got = am("/model ", None);
    assert_eq!(got.len(), 6, "{got:?}");
    let sonnet = got.iter().find(|c| c.name == "model sonnet").unwrap();
    assert!(sonnet.description.ends_with("now"), "{sonnet:?}");
    let x = am("/effort x", None);
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].name, "effort xhigh");
    assert!(am("/model", None).is_empty());
    let got = am("/model ", Some(""));
    let def = got.iter().find(|c| c.name == "model default").unwrap();
    assert_eq!(def.description, "the agent's own · now");
}

/// With enter set to send, a message ending in a command typed in full
/// sends; a partial word still completes, and by default a full one does
/// too (TestSlashFullWordSends).
#[test]
fn a_full_word_sends_when_enter_sends() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    conn(&mut m, KEY).local = vec![cmd("pdf", ""), cmd("pdf-tools", "")];
    typed(&mut m, "then run /pdf", 0);
    assert_eq!(slash::key(&mut m, &Press::of("enter").unwrap()), Out::Draw);
    assert_eq!(input(&mut m), "then run /pdf ");
    crate::screens::settings::config_mut(&mut m).enter_sends_command = true;
    typed(&mut m, "then run /pdf", 0);
    assert_eq!(slash::key(&mut m, &Press::of("enter").unwrap()), Out::Pass);
    assert_eq!(input(&mut m), "then run /pdf");
    typed(&mut m, "then run /pd", 0);
    assert_eq!(slash::key(&mut m, &Press::of("enter").unwrap()), Out::Draw);
    assert_eq!(input(&mut m), "then run /pdf ");
}

/// ↑↓ go round the picker, tab completes with a space to type after, and
/// enter on a command that is the whole message runs it: /diff opens the
/// changes view, nothing sent.
#[test]
fn keys_choose_complete_and_run() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    typed(&mut m, "/co", 0);
    press(&mut m, "up");
    assert_eq!(
        conn(&mut m, KEY).slash_sel,
        5,
        "↑ from the first goes round"
    );
    press(&mut m, "down");
    press(&mut m, "down");
    assert_eq!(conn(&mut m, KEY).slash_sel, 1);
    press(&mut m, "tab");
    assert_eq!(input(&mut m), "/context ");
    typed(&mut m, "/dif", 0);
    press(&mut m, "enter");
    assert_eq!(input(&mut m), "", "it ran");
    assert_eq!(super::views::view_name(&m, KEY), "changes");
    assert!(fake.ops().is_empty(), "{:?}", fake.ops());
}

/// A value picked from "/model " with enter switches it from the next
/// turn, said so, and kept as picked; refused by the host, the pick is
/// put back and the error said (setArg).
#[test]
fn a_model_picked_is_kept_and_put_back_when_refused() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    typed(&mut m, "/model hai", 0);
    press(&mut m, "enter");
    assert_eq!(fake.ops(), [Op::Model("haiku".into())]);
    assert_eq!(flash(&m), "model: haiku from the next turn");
    assert_eq!(conn(&mut m, KEY).picked.get("model").unwrap(), "haiku");
    // The picker marks it now, not what the answers ran.
    typed(&mut m, "/model ", 0);
    let rows: Vec<String> = slash::slash_lines(&mut m, KEY, 120)
        .iter()
        .map(Line::plain)
        .collect();
    assert!(
        rows.iter()
            .any(|r| r.contains("/model haiku") && r.contains("· now")),
        "{rows:?}"
    );
    *fake.fail.lock().unwrap() = Some("no such model".into());
    assert_eq!(
        rushcmd::run(&mut m, KEY, "/model opus"),
        Out::Draw,
        "rush's to say"
    );
    assert_eq!(
        conn(&mut m, KEY).picked.get("model").unwrap(),
        "haiku",
        "put back"
    );
    assert!(m.said.iter().any(|s| s.text.contains("no such model")));
    // default is the agent's own: none named.
    rushcmd::run(&mut m, KEY, "/effort default");
    assert_eq!(fake.ops().last(), Some(&Op::Effort(String::new())));
    assert_eq!(flash(&m), "effort: default from the next turn");
    assert_eq!(conn(&mut m, KEY).picked.get("effort").unwrap(), "");
}

/// Never picked before, a refusal leaves nothing picked.
#[test]
fn a_first_pick_refused_leaves_none() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    *fake.fail.lock().unwrap() = Some("busy".into());
    rushcmd::run(&mut m, KEY, "/effort high");
    assert!(conn(&mut m, KEY).picked.is_empty());
}

/// /model alone lists the agent's models, its own default first, the one
/// running marked; enter on one switches it (TestChoicePicker).
#[test]
fn model_alone_opens_the_list() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    assert_eq!(rushcmd::run(&mut m, KEY, "/model"), Out::Draw);
    let s = m.sheet_as::<ArgSheet>().expect("the list");
    assert_eq!(s.inner.opts[0].id, "", "the agent's own first");
    let now = s.inner.now.expect("the one running marked");
    assert_eq!(s.inner.opts[now].id, "sonnet");
    let body: Vec<String> = {
        let mut s = m.sheet.take().unwrap();
        let b = s.body(&m, 90, 30).iter().map(Line::plain).collect();
        m.sheet = Some(s);
        b
    };
    assert!(
        body[0].contains("Model") && body[0].contains("for this session, from the next turn"),
        "{body:?}"
    );
    assert!(
        body.iter()
            .any(|l| l.contains("/model <name> takes one not listed"))
    );
    m.sheet_key(&Press::of("down").unwrap());
    m.sheet_key(&Press::of("enter").unwrap());
    assert!(m.sheet.is_none());
    assert!(matches!(fake.ops().last(), Some(Op::Model(_))));
}

/// A session rush doesn't host: /model says to move it over, /plan too.
#[test]
fn not_hosted_says_so() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    conn(&mut m, KEY).link = None;
    assert_eq!(rushcmd::run(&mut m, KEY, "/model haiku"), Out::Draw);
    assert_eq!(
        flash(&m),
        "/model works in rush-mode sessions · /rush moves this one over"
    );
    rushcmd::run(&mut m, KEY, "/plan");
    assert_eq!(
        flash(&m),
        "/plan works in rush-mode sessions · /rush moves this one over"
    );
    // /compact is Claude Code's, not rush's.
    assert_eq!(rushcmd::run(&mut m, KEY, "/review"), Out::Pass);
}

/// /plan asks for plan mode, and off again once in it.
#[test]
fn plan_goes_on_and_off() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    rushcmd::run(&mut m, KEY, "/plan");
    assert_eq!(fake.ops().last(), Some(&Op::Permission("plan".into())));
    conn(&mut m, KEY).info.permission_mode = "plan".into();
    rushcmd::run(&mut m, KEY, "/plan");
    assert_eq!(fake.ops().last(), Some(&Op::Permission("default".into())));
}

/// /copy copies the last answer, /copy 2 the one before, and says how
/// many lines; with none it says so.
#[test]
fn copy_takes_the_last_answer() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    m.session.as_mut().unwrap().sess = rush_convo::Session::new();
    rushcmd::run(&mut m, KEY, "/copy");
    assert_eq!(flash(&m), "no answer to copy yet");
    let s = &mut m.session.as_mut().unwrap().sess;
    for (i, t) in ["first\nanswer", "second"].iter().enumerate() {
        s.sent(format!("ask {i}"), Vec::new(), 1_000 + i as i64);
        s.apply(
            &rush_core::event::Event::Message(rush_core::event::Message {
                role: "assistant".into(),
                id: format!("m{i}"),
                parts: vec![rush_core::event::Part {
                    kind: rush_core::event::PartKind::Text,
                    text: t.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            1_010 + i as i64,
        );
        s.apply(
            &rush_core::event::Event::TurnEnd(rush_core::event::TurnEnd {
                reason: "done".into(),
                ..Default::default()
            }),
            1_020 + i as i64,
        );
    }
    rushcmd::run(&mut m, KEY, "/copy");
    assert_eq!(m.pending_copy.as_deref(), Some("second"));
    assert_eq!(flash(&m), "copied Claude's answer · 1 lines");
    rushcmd::run(&mut m, KEY, "/copy 2");
    assert_eq!(m.pending_copy.as_deref(), Some("first\nanswer"));
    assert_eq!(flash(&m), "copied Claude's answer · 2 lines");
}

/// /background and /resume go back to Agents, saying where it is; what
/// rush leaves out says why; one the agent can't do says so.
#[test]
fn back_to_agents_and_what_isnt_in_rush() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    conn(&mut m, KEY).input.set("/bg");
    keys::send(&mut m, KEY, false);
    assert_eq!(
        flash(&m),
        "it carries on in the background · it's in Agents"
    );
    assert!(m.session.is_none(), "back to Agents");
    let (mut m, _) = session(&d);
    rushcmd::run(&mut m, KEY, "/continue");
    assert_eq!(
        flash(&m),
        "past conversations are in Agents: pick one, and a message carries it on"
    );
    let (mut m, _) = session(&d);
    rushcmd::run(&mut m, KEY, "/theme");
    assert_eq!(
        flash(&m),
        "/theme isn't in rush: it's for Claude Code's own terminal"
    );
    // The box is emptied once rush has a command.
    conn(&mut m, KEY).input.set("/vim");
    keys::send(&mut m, KEY, false);
    assert_eq!(input(&mut m), "");
}

/// /tasks shows what runs in the background; /diff the changes view.
#[test]
fn tasks_and_diff_show_their_views() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    rushcmd::run(&mut m, KEY, "/tasks");
    assert_eq!(super::views::view_name(&m, KEY), "background");
    assert_eq!(flash(&m), "");
    rushcmd::run(&mut m, KEY, "/diff");
    assert_eq!(super::views::view_name(&m, KEY), "changes");
}

/// /rush on a session rush already runs says so.
#[test]
fn rush_on_one_rush_runs() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    rushcmd::run(&mut m, KEY, "/rush");
    assert_eq!(flash(&m), "port it already runs in rush mode");
}

/// # in the box offers rush's commands for this agent, @ the agents to
/// tag; the picker's last row says how.
#[test]
fn hash_in_the_box_offers_rushs() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, _) = session(&d);
    typed(&mut m, "#expa", 0);
    let rows: Vec<String> = slash::slash_lines(&mut m, KEY, 80)
        .iter()
        .map(Line::plain)
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].contains("#expand"), "{rows:?}");
    assert!(
        rows[1].contains("rush's, for this agent · ↑↓ · tab completes · enter runs"),
        "{rows:?}"
    );
    press(&mut m, "tab");
    assert_eq!(input(&mut m), "#expand");
}

/// #model and #effort in the session open are its /model and /effort.
#[test]
fn hash_model_in_the_session_is_its_own() {
    let d = tempfile::tempdir().unwrap();
    let (mut m, fake) = session(&d);
    m.command("#model haiku");
    assert_eq!(fake.ops(), [Op::Model("haiku".into())]);
    m.command("#effort");
    assert!(m.sheet_as::<ArgSheet>().is_some(), "its list");
}

/// "/" alone in a Claude Code session lists Go's 61, in Go's order: cd,
/// btw and compact are Claude Code's now (Go's slashMatches with the
/// same session's commands and those on disk).
#[test]
fn slash_alone_lists_gos_61() {
    let d = tempfile::tempdir().unwrap();
    let (m, _) = session(&d);
    let said = [cmd("compact", ""), cmd("review", ""), cmd("context", "")];
    let local = [cmd("release-check", ""), cmd("review-pr", "")];
    let want = "clear fork rewind model effort plan diff tasks copy rename cd stop background \
        resume help btw export subtask status context usage stats skills plugin mcp hooks \
        permissions memory config statusline compact review release-check review-pr \
        claude:login claude:logout claude:upgrade claude:usage-credits \
        claude:rate-limit-options claude:privacy-settings claude:setup-bedrock \
        claude:setup-vertex claude:teleport claude:remote-control claude:session \
        claude:remote-env claude:web-setup claude:cloud-plugins claude:desktop claude:mobile \
        claude:ultraplan claude:ultrareview claude:autofix-pr claude:install-github-app \
        claude:install-slack-app claude:design-login claude:design-consent \
        claude:design-revoke claude:artifacts claude:workflows claude:daemon";
    let want: Vec<&str> = want.split_whitespace().collect();
    assert_eq!(want.len(), 61);
    assert_eq!(names(&m, &said, &local, "/", 0), want);
}
