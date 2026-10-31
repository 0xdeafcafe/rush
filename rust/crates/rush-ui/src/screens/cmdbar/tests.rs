//! The command bar's, the keys guide's and the keymap's tests, after Go's
//! internal/ui/cmdbar_test.go, keybind_test.go, commandprefix_test.go and
//! the tables behind keyboard.go, linkmenu.go and oncmd.go. No sleeps: a
//! model with no loop searches at once; transcripts are temp files.

use std::collections::{BTreeMap, HashMap, HashSet};

use rush_core::agent::{Agent, State};
use rush_core::text::{Line, width};
use rush_core::time::Ms;
use rush_keymap::File;

use super::bar::{self, BarState, ScopeKind};
use super::fuzzy::fuzzy_match;
use super::keybind::{KeyState, set_keys};
use super::keyboard::{KB_WIDE, kb_cells, kb_rows, key_parts, keyboard};
use super::keyhelp::{self, HelpState, pretty_key};
use super::linkmenu::{LinkAct, link_acts, previewable};
use super::oncmd::on_start;
use super::{draw, fleetslash};
use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::worker::Fleet;

const NOW: Ms = 1_793_190_896_789;
const MIN: Ms = 60_000;

fn agent(id: &str, name: &str, state: State, pid: i32, ago: Ms) -> Agent {
    let mut a = Agent {
        key: format!("default/{id}"),
        display_name: name.into(),
        repo: "/src/checkout".into(),
        branch: "main".into(),
        pid,
        interactive: true,
        ..Agent::default()
    };
    a.kind = rush_core::adapter::Kind::new("claude");
    a.job.id = id.into();
    a.job.state = state;
    a.job.cwd = "/src/checkout".into();
    a.job.created_at = NOW - ago - 20 * MIN;
    a.job.updated_at = NOW - ago;
    a.spend.first = NOW - ago - 20 * MIN;
    a.spend.last = NOW - ago;
    a.spend.ready = true;
    a
}

fn fixture() -> Vec<Agent> {
    let mut ask = agent(
        "a1",
        "fix the flaky tax rounding test",
        State::Blocked,
        101,
        2 * MIN,
    );
    ask.job.needs = "asks: Round half up, or half to even?".into();
    vec![
        ask,
        agent("b2", "add Apple Pay to checkout", State::Working, 102, MIN),
        agent(
            "c3",
            "move the design tokens to CSS",
            State::Working,
            103,
            MIN / 2,
        ),
        agent("d4", "set up caching for CI", State::Idle, 104, 10 * MIN),
        agent(
            "e5",
            "rename Invoice to Bill everywhere",
            State::Done,
            0,
            72 * 60 * MIN,
        ),
    ]
}

fn model(w: usize, h: usize) -> Model {
    // A registry of its own: one Claude put in the process's would reach
    // every other test's model.
    let reg = rush_core::adapter::Registry::default();
    reg.register(rush_claude::Claude::new());
    let mut m = Model::new("0.1.0");
    m.reg = reg;
    m.clock = Some(NOW);
    m.launch_dir = "/src/checkout".into();
    m.w = w;
    m.h = h;
    m.set_fleet(Fleet {
        agents: fixture(),
        roots: vec!["/src/checkout".into(); 5],
        previews: HashMap::new(),
        priced: true,
    });
    m
}

fn press(m: &mut Model, s: &str) -> Out {
    m.press_key(&Press::of(s).unwrap_or_else(|| panic!("no key {s:?}")))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        let k = if c == ' ' {
            "space".to_string()
        } else {
            c.to_string()
        };
        press(m, &k);
    }
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(Line::plain).collect()
}

fn the_bar(m: &Model) -> &bar::Bar {
    m.kept
        .peek::<BarState>()
        .and_then(|s| s.bar.as_ref())
        .expect("the bar is open")
}

fn sections(m: &Model) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for it in &the_bar(m).items {
        if !out.contains(&it.section) {
            out.push(it.section.clone());
        }
    }
    out
}

// ---- cmdbar_test.go ----

#[test]
fn bar_opens_and_goes() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    assert!(bar::is_open(&m), "ctrl+k didn't open the bar");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("✻ Go anywhere") && s.contains("❯"), "{s}");
    typed(&mut m, "harn");
    // Harnesses has pages, each its own row, the first first (cmdbar.go:773).
    assert_eq!(the_bar(&m).items[0].title, "Harnesses › Providers");
    press(&mut m, "enter");
    assert!(!bar::is_open(&m));
    assert_eq!(m.place, PlaceId::Harnesses);
    // Back returns where it came from.
    press(&mut m, "ctrl+k");
    typed(&mut m, "back");
    assert_eq!(the_bar(&m).items[0].title, "Back");
    press(&mut m, "enter");
    assert_eq!(m.place, PlaceId::Agents);
}

#[test]
fn ctrl_k_still_cuts() {
    let mut m = model(120, 40);
    m.ext.agents_box = true;
    m.input.insert_str("hello");
    m.input.pos = 0; // text after the cursor
    press(&mut m, "ctrl+k");
    assert!(
        !bar::is_open(&m),
        "ctrl+k opened the bar instead of cutting"
    );
}

#[test]
fn bar_finds_an_agent() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    typed(&mut m, "tax round");
    let b = the_bar(&m);
    assert!(
        b.items
            .iter()
            .any(|it| it.section == "Agents" && it.title == "fix the flaky tax rounding test"),
        "{:?}",
        b.items
    );
    // Anything typed can be a new agent's task.
    assert!(
        b.items
            .iter()
            .any(|it| it.section == "Start" && it.title == "tax round")
    );
}

#[test]
fn bar_untyped_lists_agents_by_need() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    let secs = sections(&m);
    assert_eq!(secs[0], "Needs you", "{secs:?}");
    assert!(secs.contains(&"Working".to_string()));
    assert!(
        the_bar(&m)
            .items
            .iter()
            .any(|it| it.title == "Start an agent")
    );
    // ctrl+s orders them by the latest instead.
    press(&mut m, "ctrl+s");
    assert_eq!(sections(&m)[0], "Latest");
}

#[test]
fn bar_hash_lists_commands() {
    let mut m = model(120, 40);
    m.sel.clear();
    m.shown.clear();
    press(&mut m, "ctrl+k");
    typed(&mut m, "#");
    let b = the_bar(&m);
    assert!(b.items.iter().all(|it| it.section.starts_with("Commands")));
    assert!(b.items.iter().any(|it| it.title == "#use"));
    assert!(
        !b.items.iter().any(|it| it.title == "#done"),
        "#done with no agent focused"
    );
    assert!(!b.items.iter().any(|it| it.section == "Start"));
    // #help runs at once, the way typing it would.
    bar::close(&mut m);
    press(&mut m, "ctrl+k");
    typed(&mut m, "#help");
    let i = the_bar(&m)
        .items
        .iter()
        .position(|it| it.title == "#help")
        .expect("#help");
    m.kept::<BarState>().bar.as_mut().unwrap().cursor = i;
    press(&mut m, "enter");
    assert!(keyhelp::is_open(&m), "#help didn't open the keys");
}

#[test]
fn bar_scopes() {
    let mut m = model(120, 40);
    m.sel = "default/a1".into(); // needs you
    let group = m.group_of[&m.sel].clone();
    press(&mut m, "ctrl+f");
    let sc = the_bar(&m).scope().clone();
    assert_eq!(
        (sc.kind, sc.group.as_str()),
        (ScopeKind::Group, group.as_str())
    );
    assert!(!the_bar(&m).items.is_empty());
    assert!(the_bar(&m).items.iter().all(|it| it.section == group));
    press(&mut m, "ctrl+f");
    assert_eq!(the_bar(&m).scope().kind, ScopeKind::Agents);
    press(&mut m, "ctrl+k");
    assert_eq!(the_bar(&m).scope().kind, ScopeKind::All);
    press(&mut m, "ctrl+f");
    assert_eq!(the_bar(&m).scope().kind, ScopeKind::Group, "round again");
    // Backspace with nothing typed widens too.
    press(&mut m, "backspace");
    assert_eq!(the_bar(&m).scope().kind, ScopeKind::Agents);
    press(&mut m, "ctrl+k");
    press(&mut m, "ctrl+k");
    assert!(!bar::is_open(&m), "ctrl+k at everywhere should close");
}

#[test]
fn bar_place_scope() {
    let mut m = model(120, 40);
    m.set_place(PlaceId::Settings);
    press(&mut m, "ctrl+f");
    let b = the_bar(&m);
    assert_eq!(b.scope().kind, ScopeKind::Place);
    assert_eq!(b.scope().label, "Settings");
    let s = screen(&mut m).join("\n");
    assert!(
        s.contains("⌕ Find in Settings") && s.contains("ctrl+f"),
        "{s}"
    );
}

/// The frame, its labels and its shadow keep every row the screen's
/// width, glint and all (TestBarFrameWidth).
#[test]
fn bar_frame_width() {
    for (w, h) in [(80, 24), (130, 40), (250, 70)] {
        let mut m = model(w, h);
        press(&mut m, "ctrl+k");
        {
            let b = m.kept::<BarState>().bar.as_mut().unwrap();
            b.search = Some(super::search::Search::idle());
            b.to_search = 80;
        }
        for (i, l) in m.frame().iter().enumerate() {
            assert_eq!(l.width(), w, "{w}x{h} row {i}: {:?}", l.plain());
        }
        let s = screen(&mut m).join("\n");
        assert!(s.contains("searching 0/80 conversations"), "{s}");
    }
}

#[test]
fn bar_frame_at_go_widths() {
    // As Go's --render 120x36 ctrl+k draws it over the list: the frame a
    // sixth of the way down, 110 wide, centred, its hint at the foot.
    let mut m = model(120, 36);
    press(&mut m, "ctrl+k");
    let s = screen(&mut m);
    let top = s
        .iter()
        .position(|l| l.contains("╭─ ✻ Go anywhere"))
        .expect("top edge");
    assert_eq!(top, 4);
    assert_eq!(
        s[top].find('╭').map(|i| s[top][..i].chars().count()),
        Some(4)
    );
    assert!(s[top].trim_end().ends_with("ctrl+k ─╮"), "{}", s[top]);
    assert!(
        s[top + 1]
            .contains("❯ ▏an agent, words said in any conversation, or a place · # for commands")
    );
    let foot = s.iter().position(|l| l.contains("╰")).expect("bottom edge");
    assert!(
        s[foot - 1].contains("↑↓ choose  ·  enter go"),
        "{}",
        s[foot - 1]
    );
    assert!(s[foot - 1].contains("esc close"));
}

#[test]
fn bar_hint_teaches_scopes() {
    let mut m = model(120, 40);
    m.sel = "default/a1".into();
    press(&mut m, "ctrl+f");
    let hint = |m: &Model| {
        let b = the_bar(m);
        draw::hint(b, "", 200).plain()
    };
    assert!(
        hint(&m).contains("ctrl+f wider: all agents"),
        "{}",
        hint(&m)
    );
    assert!(hint(&m).contains("ctrl+k everywhere"));
    press(&mut m, "ctrl+f");
    assert!(hint(&m).contains("ctrl+f · ctrl+k everywhere"));
    press(&mut m, "ctrl+f");
    assert!(hint(&m).contains("ctrl+f just "), "{}", hint(&m));
    assert!(hint(&m).contains("ctrl+s by status"));
}

#[test]
fn bar_searches_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = model(120, 40);
    let line = r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"rotate the zebracorn keys"},"origin":{"kind":"human"}}"#;
    let mut agents = fixture();
    for (i, a) in agents.iter_mut().enumerate() {
        let p = dir.path().join(format!("{}.jsonl", a.job.id));
        let text = if i == 2 { line } else { "nothing to see" };
        std::fs::write(&p, format!("{text}\n")).unwrap();
        a.job.transcript_path = p.to_string_lossy().into_owned();
    }
    m.set_fleet(Fleet {
        agents,
        roots: vec!["/src/checkout".into(); 5],
        previews: HashMap::new(),
        priced: true,
    });
    press(&mut m, "ctrl+k");
    typed(&mut m, "zebracorn");
    // With no loop the pause never ends, as in a single Go frame…
    assert!(the_bar(&m).search.is_none());
    assert!(screen(&mut m).join("\n").contains(" … "));
    // …and ctrl+j searches at once.
    press(&mut m, "ctrl+j");
    let b = the_bar(&m);
    assert!(
        b.done && b.searched == 5,
        "searched {} of {}",
        b.searched,
        b.to_search
    );
    let hit = b
        .items
        .iter()
        .find(|it| it.section == "Conversations")
        .expect("a transcript hit")
        .clone();
    assert!(hit.title.contains("zebracorn"), "{}", hit.title);
    let s = screen(&mut m).join("\n");
    assert!(s.contains("1 found in 1 of 5 conversations"), "{s}");
    // The agent that said it is offered too.
    assert!(
        the_bar(&m)
            .items
            .iter()
            .any(|it| it.meta.starts_with("said “rotate the zebracorn keys”"))
    );
    // Going there selects the agent and waits for its conversation.
    let go = hit.go.clone();
    bar::close(&mut m);
    bar::run(&mut m, &go);
    assert_eq!(m.sel, "default/c3");
    assert!(m.session.is_some());
}

#[test]
fn fuzzy_table() {
    for (q, s, ok) in [
        ("pc", "Agents › Projects › Cleanup", true),
        ("setpro", "Settings › Providers", true),
        ("zz", "Settings › Providers", false),
        ("clean", "Agents › Projects › Cleanup", true),
    ] {
        assert_eq!(fuzzy_match(q, s).is_some(), ok, "fuzzy({q:?}, {s:?})");
    }
    // A word start beats the middle of a word.
    let a = fuzzy_match("proc", "Agents › Projects › Processes")
        .unwrap()
        .0;
    let b = fuzzy_match("proc", "reprocess").unwrap().0;
    assert!(a > b, "word start {a} should beat mid-word {b}");
}

#[test]
fn words_and_times() {
    assert_eq!(
        bar::split_in("fix in:tax it"),
        ("fix it".into(), "tax".into())
    );
    assert!(bar::searchable("abc"));
    assert!(!bar::searchable("ab in:zzzz"));
    for (d, want) in [
        (30_000, "just now"),
        (5 * MIN, "5m ago"),
        (3 * 60 * MIN, "3h ago"),
    ] {
        assert_eq!(bar::when_ago(NOW - d, NOW), want);
    }
    assert_eq!(bar::when_ago(NOW - 30 * 60 * MIN, NOW), "yesterday");
    assert_eq!(bar::when_ago(0, NOW), "");
    assert_eq!(
        bar::lit_words("rotate the keys", &["keys".into()]),
        vec![11, 12, 13, 14]
    );
}

#[test]
fn bar_edge_glows_and_glints() {
    let e = draw::Edge {
        w: 20,
        h: 6,
        glint: None,
    };
    assert_eq!(e.color(0, 0), rush_core::text::Color::Rgb(232, 128, 92));
    assert_eq!(e.color(19, 5), rush_core::text::Color::Rgb(110, 52, 40));
    let g = draw::Edge {
        glint: Some(5),
        ..e
    };
    assert_ne!(g.color(5, 0), e.color(5, 0), "the glint lights its cell");
    assert_eq!(g.color(19, 5), e.color(19, 5), "and leaves the far side");
    let l = e.line(0, "╭", "╮", Line::new(), Line::new());
    assert_eq!(l.width(), 20);
}

// ---- keyhelp.go, keybind_test.go ----

#[test]
fn keys_guide() {
    let mut m = model(200, 50);
    press(&mut m, "f1");
    assert!(keyhelp::is_open(&m), "F1 didn't open the keys");
    let first = |m: &Model| {
        m.kept
            .peek::<HelpState>()
            .unwrap()
            .open
            .as_ref()
            .unwrap()
            .rows[0]
            .section
            .clone()
    };
    assert_eq!(first(&m), "Here · Agents and the Prompt");
    let s = screen(&mut m).join("\n");
    assert!(
        s.contains("⌨ Keys") && s.contains("find a key by its keys or what it does"),
        "{s}"
    );
    assert!(s.contains("Everywhere") || s.contains("the agent above"));
    typed(&mut m, "quit rush");
    let g = m.kept.peek::<HelpState>().unwrap().open.as_ref().unwrap();
    assert_eq!(g.rows[g.cur].id, "quit");
    press(&mut m, "esc");
    assert!(keyhelp::is_open(&m), "esc first clears what's typed");
    press(&mut m, "esc");
    assert!(!keyhelp::is_open(&m));
    // ? in the list with nothing typed, and while typing in the Prompt.
    press(&mut m, "?");
    assert!(keyhelp::is_open(&m));
    press(&mut m, "f1");
    assert!(!keyhelp::is_open(&m));
    m.ext.agents_box = true;
    m.input.insert_str("list draft");
    press(&mut m, "f1");
    assert_eq!(first(&m), "Here · The Prompt, with something typed");
    press(&mut m, "f1");
    assert_eq!(m.input.string(), "list draft", "the draft is kept");
    // enter takes you to Settings, Keys on the key.
    m.input.clear();
    press(&mut m, "f1");
    typed(&mut m, "command bar");
    press(&mut m, "enter");
    assert!(!keyhelp::is_open(&m));
    assert_eq!(m.place, PlaceId::Settings);
    assert_eq!(m.kept::<keyhelp::ShowKey>().0.as_deref(), Some("bar.open"));
    // F1 from a place too.
    press(&mut m, "f1");
    assert!(keyhelp::is_open(&m));
}

#[test]
fn keys_guide_frame_width() {
    for (w, h) in [(80, 24), (120, 36), (250, 70)] {
        let mut m = model(w, h);
        press(&mut m, "f1");
        for (i, l) in m.frame().iter().enumerate() {
            assert_eq!(l.width(), w, "{w}x{h} row {i}: {:?}", l.plain());
        }
    }
}

#[test]
fn pretty_keys() {
    let mac = cfg!(target_os = "macos");
    for (k, want) in [
        ("super+k", "⌘k"),
        ("ctrl+up", "ctrl+↑"),
        ("ctrl+x h", "ctrl+x h"),
        ("shift+down", "shift+↓"),
        ("+", "+"),
        ("alt+m", if mac { "⌥m" } else { "alt+m" }),
    ] {
        assert_eq!(pretty_key(k), want, "{k}");
    }
    assert_eq!(
        keyhelp::key_text(&["home".into(), "shift+up".into()]),
        "home · shift+↑"
    );
}

#[test]
fn remap_moves_a_key() {
    let mut m = model(120, 40);
    let mut b = BTreeMap::new();
    b.insert("bar.open".to_string(), vec!["ctrl+g".to_string()]);
    set_keys(&mut m, File { bindings: b });
    press(&mut m, "ctrl+g");
    assert!(bar::is_open(&m), "ctrl+g bound to the bar didn't open it");
    bar::close(&mut m);
    // ctrl+k no longer does anything: it doesn't type.
    press(&mut m, "ctrl+k");
    assert!(!bar::is_open(&m));
}

#[test]
fn chord_runs_an_action() {
    let mut m = model(120, 40);
    let mut b = BTreeMap::new();
    b.insert("guide.open".to_string(), vec!["ctrl+x h".to_string()]);
    set_keys(&mut m, File { bindings: b });
    assert_eq!(press(&mut m, "ctrl+x"), Out::Draw);
    assert!(!keyhelp::is_open(&m));
    assert_eq!(m.kept::<KeyState>().chord.to_string(), "ctrl+x");
    press(&mut m, "h");
    assert!(keyhelp::is_open(&m), "the chord didn't open the keys");
    keyhelp::close(&mut m);
    // A chord no action finishes says so.
    press(&mut m, "ctrl+x");
    press(&mut m, "z");
    let f = m.flash.clone().expect("a flash");
    assert_eq!(
        (f.0.as_str(), f.2),
        ("ctrl+x z does nothing", false),
        "a refusal, not red"
    );
}

/// ctrl+] f opens the list's filter, as alt+f does, and a chord that ran
/// takes its "ctrl+] …" off the line under the box (keybind.go:122).
#[test]
fn chord_filter_and_its_flash() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+]");
    assert_eq!(m.flash.as_ref().map(|f| f.0.as_str()), Some("ctrl+] …"));
    press(&mut m, "f");
    assert!(
        m.kept
            .peek::<crate::screens::list_extras::filter::FilterState>()
            .is_some_and(|f| f.open),
        "ctrl+] f didn't open the filter"
    );
    assert!(m.input.is_empty(), "the f was typed");
    assert!(m.flash.is_none(), "still says ctrl+] …: {:?}", m.flash);
}

/// ctrl+] ] and ctrl+] [ step through the chats in play; ctrl+tab too
/// (keybind.go:158 stepChat).
#[test]
fn chord_steps_chats() {
    let mut m = model(120, 40);
    let first = m.sel.clone();
    press(&mut m, "ctrl+]");
    press(&mut m, "]");
    let next = m.sel.clone();
    assert_ne!(next, first, "ctrl+] ] stayed put");
    press(&mut m, "ctrl+]");
    press(&mut m, "[");
    assert_eq!(m.sel, first, "ctrl+] [ didn't come back");
    press(&mut m, "ctrl+tab");
    assert_eq!(m.sel, next, "ctrl+tab");
}

/// ctrl+] | pins the open agent as a tile, ctrl+] k takes it off
/// (keybind.go:167-170 gridPin, gridUnpin).
#[test]
fn chord_pins_a_tile() {
    use crate::screens::list_extras::grid::Grid;
    let mut m = model(160, 40);
    let key = m.agents[1].key.clone();
    m.select(key.clone());
    m.open(1);
    press(&mut m, "ctrl+]");
    press(&mut m, "|");
    assert_eq!(m.kept::<Grid>().keys, vec![key.clone()]);
    assert_eq!(m.kept::<Grid>().under.get(&key), Some(&false));
    let f = m.flash.clone().expect("a flash").0;
    assert!(f.contains("pinned beside the Session"), "{f}");
    press(&mut m, "ctrl+]");
    press(&mut m, "-");
    assert_eq!(m.kept::<Grid>().under.get(&key), Some(&true), "ctrl+] -");
}

/// ctrl+] a adds what's typed in the Prompt to the open agent's box
/// (keybind.go:179 promptToAgent).
#[test]
fn chord_moves_the_prompt_to_the_agent() {
    let mut m = model(160, 40);
    m.agents[3].rush = true;
    let key = m.agents[3].key.clone();
    m.open(3);
    crate::screens::session_core::conn(&mut m, &key).input.text = "first".chars().collect();
    m.input.text = "second".chars().collect();
    super::keybind::run_action(&mut m, "prompt.append");
    let got: String = crate::screens::session_core::conn(&mut m, &key)
        .input
        .text
        .iter()
        .collect();
    assert_eq!(got, "first\nsecond");
    assert!(m.input.is_empty(), "the Prompt kept it");
    m.input.text = "third".chars().collect();
    super::keybind::run_action(&mut m, "prompt.replace");
    let got: String = crate::screens::session_core::conn(&mut m, &key)
        .input
        .text
        .iter()
        .collect();
    assert_eq!(got, "third");
}

// ---- commandprefix_test.go ----

/// #agent, #handoff, #profile and #preset are now #use and #fork.
#[test]
fn renamed_command_aliases() {
    for (old, now) in [
        ("agent", "use"),
        ("handoff", "fork"),
        ("profile", "use"),
        ("preset", "use"),
    ] {
        assert_eq!(fleetslash::alias_of(old), now, "#{old}");
        assert!(fleetslash::is_fleet_command(old), "#{old}");
    }
}

/// Every # command is an action a key can be bound to.
#[test]
fn commands_are_actions() {
    let acts = super::keybind::key_actions();
    for (name, _, _) in fleetslash::FLEET_COMMANDS {
        let id = rush_keymap::command_id(name);
        assert!(acts.iter().any(|a| a.id == id), "{id}");
    }
}

// ---- keyboard.go, linkmenu.go, oncmd.go ----

#[test]
fn keyboard_parts() {
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
    for (seq, want) in [
        (vec!["ctrl+k"], set(&["ctrl", "k"])),
        (vec!["?"], set(&["shift", "/"])),
        (vec!["G"], set(&["shift", "g"])),
        (vec!["ctrl+x", "h"], set(&["ctrl", "x", "h"])),
        (vec!["alt+shift+up"], set(&["alt", "shift", "up"])),
    ] {
        let seq: Vec<String> = seq.into_iter().map(String::from).collect();
        assert_eq!(key_parts(&seq), want, "{seq:?}");
    }
}

#[test]
fn keyboard_fits() {
    // The bottom row runs a cell past, as Go's does: the drawing cuts it.
    for row in kb_rows() {
        let w: usize = row.iter().map(|k| k.w + 1).sum::<usize>() - 1;
        assert!(w <= KB_WIDE + 1, "a row {w} wide");
    }
    let lit: HashSet<String> = ["k", "ctrl"].iter().map(|s| s.to_string()).collect();
    let none = HashSet::new();
    let cells = kb_cells(&none, &lit, &none);
    assert!(
        cells
            .iter()
            .any(|c| c.c == 'k' && c.face == super::keyboard::kb_faces()[1])
    );
    for l in keyboard(&none, &lit, &none) {
        assert!(width(&l.plain()) <= KB_WIDE);
    }
}

#[test]
fn link_menu_acts() {
    assert_eq!(
        link_acts("https://example.com/a"),
        vec![
            LinkAct::Browse("https://example.com/a".into()),
            LinkAct::CopyLink("https://example.com/a".into())
        ]
    );
    let acts = link_acts("file:///tmp/a%20b.png");
    assert_eq!(acts[0], LinkAct::Open("/tmp/a b.png".into()));
    assert_eq!(acts[1], LinkAct::Preview("/tmp/a b.png".into()));
    assert_eq!(acts.len(), 5);
    assert_eq!(
        link_acts("file://host/x.txt").len(),
        4,
        "no Preview for text"
    );
    assert!(link_acts("two words").is_empty());
    assert!(previewable("A.PDF") && !previewable("a.go"));
}

#[test]
fn on_reads_its_argument() {
    let peers = ["studio", "laptop"];
    let (p, r) = on_start(&peers, "Studio ~/src agent=codex fix it").unwrap();
    assert_eq!(
        (
            p.as_str(),
            r.cwd.as_str(),
            r.agent.as_str(),
            r.prompt.as_str()
        ),
        ("studio", "~/src", "codex", "fix it")
    );
    let (_, r) = on_start(&peers, "laptop run the tests").unwrap();
    assert_eq!((r.cwd.as_str(), r.prompt.as_str()), ("~", "run the tests"));
    assert_eq!(
        on_start(&peers, "").unwrap_err(),
        "#on MACHINE [folder] [agent=KIND] task"
    );
    assert_eq!(
        on_start(&peers, "nas x").unwrap_err(),
        "no machine nas · rush remote add it first"
    );
    assert_eq!(
        on_start(&peers, "studio /tmp").unwrap_err(),
        "#on studio: say what the agent should do"
    );
}

#[test]
fn off_read_takes_once() {
    let r = super::offui::OffRead::<u32>::default();
    assert!(r.start(|| 7));
    assert!(!r.start(|| 8), "one read at a time");
    r.wait();
    assert_eq!(r.take(), Some(7));
    assert_eq!(r.take(), None);
    assert!(!r.out());
}

// ---- more of cmdbar_test.go ----

/// Untyped, agents come first; what's typed can start one, but a #
/// command never does; Start an agent puts the keys in the Agents box
/// (TestBarStartsAnAgent).
#[test]
fn bar_starts_an_agent() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    assert_ne!(the_bar(&m).items[0].section, "Go to");
    assert!(
        the_bar(&m)
            .items
            .iter()
            .any(|it| it.title == "Start an agent")
    );
    typed(&mut m, "zqxj fix the flaky test");
    let start = the_bar(&m).items.iter().find(|it| it.section == "Start");
    assert_eq!(
        start.map(|it| it.title.as_str()),
        Some("zqxj fix the flaky test")
    );
    bar::close(&mut m);
    press(&mut m, "ctrl+k");
    typed(&mut m, "#done");
    assert!(!the_bar(&m).items.iter().any(|it| it.section == "Start"));
    bar::close(&mut m);
    m.set_place(PlaceId::Settings);
    press(&mut m, "ctrl+k");
    let i = the_bar(&m)
        .items
        .iter()
        .position(|it| it.title == "Start an agent")
        .unwrap();
    m.kept::<BarState>().bar.as_mut().unwrap().cursor = i;
    press(&mut m, "enter");
    assert!(!bar::is_open(&m));
    assert_eq!(m.place, PlaceId::Agents);
    assert!(m.session.is_none());
}

/// A name matches by its words, as typed or a typo away, not by letters
/// scattered through it; a place named as typed comes first
/// (TestBarMatchesNamesByWords).
#[test]
fn bar_matches_names_by_words() {
    let mut m = model(120, 40);
    let mut agents = fixture();
    for (a, name) in agents.iter_mut().zip([
        "zod performance optimization",
        "Paste Images And Codex Agent View",
        "Index Out Of Range Panic",
        "fix the indxe page",
    ]) {
        a.display_name = name.into();
    }
    m.set_fleet(Fleet {
        agents,
        roots: vec!["/src/checkout".into(); 5],
        previews: HashMap::new(),
        priced: true,
    });
    press(&mut m, "ctrl+k");
    typed(&mut m, "index");
    let found: Vec<&str> = the_bar(&m)
        .items
        .iter()
        .filter(|it| it.section == "Agents")
        .map(|it| it.title.as_str())
        .collect();
    assert_eq!(found, ["Index Out Of Range Panic", "fix the indxe page"]);
    bar::close(&mut m);
    press(&mut m, "ctrl+k");
    typed(&mut m, "settings");
    let it = &the_bar(&m).items[0];
    assert_eq!(it.section, "Go to");
    assert!(it.title.starts_with("Settings"), "{}", it.title);
}

/// Projects and its pages are offered as Go names them, whether or not
/// the Projects place is here yet.
#[test]
fn bar_offers_projects_pages() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    typed(&mut m, "proj");
    let titles: Vec<&str> = the_bar(&m)
        .items
        .iter()
        .filter(|it| it.section == "Go to")
        .map(|it| it.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "Projects",
            "Projects › Temporary",
            "Projects › System",
            "Projects › Pull requests"
        ]
    );
}

/// Who said it comes before what was said, the closest first, a typo
/// after the word as typed, each match with its times
/// (TestBarFindsAgentsThatSaidIt, TestBarRanksByRelevanceWithTimes).
#[test]
fn bar_finds_agents_that_said_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = model(140, 40);
    let said = |w: &str, at: &str| {
        format!(
            r#"{{"type":"user","timestamp":"{at}","cwd":"/work","message":{{"role":"user","content":"rotate the {w} keys"}},"origin":{{"kind":"human"}}}}"#
        )
    };
    let mut agents = fixture();
    for (i, a) in agents.iter_mut().enumerate() {
        let p = dir.path().join(format!("{}.jsonl", a.job.id));
        let text = match i {
            1 => said("zebracron", "2026-09-24T20:00:00Z"), // a typo, more recent
            3 => said("zebracorn", "2026-09-23T20:00:00Z"),
            _ => "nothing to see".into(),
        };
        std::fs::write(&p, format!("{text}\n")).unwrap();
        a.job.transcript_path = p.to_string_lossy().into_owned();
    }
    m.set_fleet(Fleet {
        agents,
        roots: vec!["/src/checkout".into(); 5],
        previews: HashMap::new(),
        priced: true,
    });
    press(&mut m, "ctrl+k");
    typed(&mut m, "zebracorn");
    press(&mut m, "ctrl+j");
    let b = the_bar(&m);
    let agents: Vec<&bar::Item> = b.items.iter().filter(|it| it.section == "Agents").collect();
    let convs: Vec<&bar::Item> = b
        .items
        .iter()
        .filter(|it| it.section == "Conversations")
        .collect();
    assert_eq!(
        agents.iter().map(|it| it.key.as_str()).collect::<Vec<_>>(),
        ["default/d4", "default/b2"],
        "{:?}",
        b.items
    );
    assert!(agents.iter().all(|it| it.tail == "1 found"));
    assert_eq!(convs.len(), 2, "{:?}", b.items);
    assert!(convs[0].title.contains("zebracorn") && convs[1].title.contains("zebracron"));
    assert!(convs[0].tail.starts_with("sent "), "{}", convs[0].tail);
    assert!(
        convs[0].meta.contains("started ") && convs[0].meta.contains("updated "),
        "{}",
        convs[0].meta
    );
    assert!(!convs[1].lit.is_empty(), "the typo isn't lit");
    let first = |s: &str| b.items.iter().position(|it| it.section == s).unwrap();
    assert!(first("Agents") < first("Conversations") && first("Conversations") < first("Start"));
    assert_eq!(b.items[b.cursor].section, "Agents");
}

/// #full and #native need the agent's own screen; #pin an agent that
/// keeps pins (availableFleetCommands).
#[test]
fn bar_commands_follow_what_the_agent_can_do() {
    let mut m = model(120, 40);
    m.sel = "default/b2".into();
    // A terminal session never goes full screen (fleetslash.go:421): one
    // rush started does, where its agent has a screen.
    for a in m.agents.iter_mut().filter(|a| a.key == "default/b2") {
        a.interactive = false;
    }
    let names: Vec<String> = fleetslash::available_fleet_commands(&m)
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert!(names.iter().any(|n| n == "done"));
    let screen = m
        .reg
        .supports("claude", rush_core::adapter::Feature::Screen);
    assert_eq!(names.iter().any(|n| n == "full"), screen, "{names:?}");
}

/// The bar and the keys guide draw nothing they can't fit, down to a
/// screen one cell wide.
#[test]
fn bar_and_guide_at_tiny_sizes() {
    for (w, h) in [(1, 1), (5, 3), (9, 2), (10, 5), (20, 8)] {
        let mut m = model(w, h);
        press(&mut m, "ctrl+k");
        typed(&mut m, "日本語");
        let _ = m.frame();
        bar::close(&mut m);
        press(&mut m, "f1");
        let _ = m.frame();
    }
}

/// The guide shows the keys in force, "no key" for an action unbound, and
/// F1 opens it in every place (TestContextualShortcutGuide).
#[test]
fn keys_guide_shows_the_keymap() {
    let mut m = model(200, 50);
    let mut b = BTreeMap::new();
    b.insert("bar.open".to_string(), vec!["ctrl+x h".to_string()]);
    b.insert("quit".to_string(), Vec::new());
    set_keys(&mut m, File { bindings: b });
    press(&mut m, "f1");
    typed(&mut m, "command bar");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("ctrl+x h"), "{s}");
    press(&mut m, "esc");
    typed(&mut m, "quit rush");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("no key"), "{s}");
    press(&mut m, "esc");
    press(&mut m, "esc");
    for p in [PlaceId::Projects, PlaceId::Harnesses, PlaceId::Settings] {
        m.set_place(p);
        press(&mut m, "f1");
        assert!(keyhelp::is_open(&m), "F1 in {p:?}");
        press(&mut m, "esc");
        assert!(!keyhelp::is_open(&m));
    }
}

/// Lighting words in titles of wide and folded chars stays on char
/// boundaries.
#[test]
fn lit_words_wide_and_folded() {
    let words: Vec<String> = ["istanbul", "日本", "zebra", "ẞtraße", "x"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for s in [
        "İstanbul ☃ zebra日本",
        "ﬃ ẞTRASSE straße",
        "日本語テキスト",
        "",
        "x",
    ] {
        let lit = bar::lit_words(s, &words);
        let n = s.chars().count();
        assert!(lit.iter().all(|&i| i < n), "{s:?} {lit:?}");
        let _ = draw::lit_title(s, &lit, 5);
    }
}

/// A chord pressed in an open Session that runs nothing visible takes its
/// "ctrl+] …" off the foot too (keybind.go:122).
#[test]
fn chord_in_session_ends_its_flash() {
    let mut m = model(160, 40);
    m.open(1);
    press(&mut m, "ctrl+]");
    assert_eq!(m.flash.as_ref().map(|f| f.0.as_str()), Some("ctrl+] …"));
    let o = m.press_key(&Press::of("c").unwrap());
    assert!(m.flash.is_none(), "still says: {:?}", m.flash);
    assert_eq!(o, Out::Draw, "the foot isn't drawn again");
}

/// ctrl+] u then ctrl+] a: the draft moved to the Prompt goes back to the
/// agent it came from, though the Session gave the keys up (keybind.go:333).
#[test]
fn draft_to_prompt_and_back() {
    let mut m = model(160, 40);
    m.agents[3].rush = true;
    let key = m.agents[3].key.clone();
    m.select(key.clone());
    m.open(3);
    crate::screens::session_core::conn(&mut m, &key).input.text = "draft".chars().collect();
    press(&mut m, "ctrl+]");
    press(&mut m, "u");
    assert_eq!(m.input.text.iter().collect::<String>(), "draft");
    press(&mut m, "ctrl+]");
    press(&mut m, "a");
    let got: String = crate::screens::session_core::conn(&mut m, &key)
        .input
        .text
        .iter()
        .collect();
    assert_eq!(got, "draft", "{:?}", m.flash);
    assert!(m.input.is_empty());
}

/// Find in a place offers that place's pages, not the place you're
/// already on (barRebuild's place scope, cmdbar.go:622-628).
#[test]
fn bar_place_scope_leaves_out_the_place() {
    let mut m = model(120, 40);
    m.set_place(PlaceId::Projects);
    press(&mut m, "ctrl+f");
    let titles: Vec<&str> = the_bar(&m)
        .items
        .iter()
        .map(|it| it.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "Projects › Temporary",
            "Projects › System",
            "Projects › Pull requests"
        ]
    );
}

/// With "ctrl+k searches transcripts" on ctrl+enter, typing matches names
/// only and the head says how to search (cmdbar.go:285, :1426-1427,
/// :1531).
#[test]
fn bar_searches_transcripts_on_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = model(120, 40);
    let line = r#"{"type":"user","timestamp":"2026-09-23T20:00:00Z","cwd":"/work","message":{"role":"user","content":"rotate the zebracorn keys"},"origin":{"kind":"human"}}"#;
    let mut agents = fixture();
    for a in agents.iter_mut() {
        let p = dir.path().join(format!("{}.jsonl", a.job.id));
        std::fs::write(&p, format!("{line}\n")).unwrap();
        a.job.transcript_path = p.to_string_lossy().into_owned();
    }
    m.set_fleet(Fleet {
        agents,
        roots: vec!["/src/checkout".into(); 5],
        previews: HashMap::new(),
        priced: true,
    });
    crate::screens::settings::store(&mut m)
        .config
        .search_transcripts_on_key = true;
    press(&mut m, "ctrl+k");
    typed(&mut m, "zebracorn");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("ctrl+j search conversations"), "{s}");
    assert!(!s.contains(" … "), "{s}");
    assert_eq!(sections(&m), ["Start"]);
    // A terminal that tells ctrl+enter from enter is told ctrl+enter
    // (searchKey, cmdbar.go:308).
    m.keys_disambiguated = true;
    let s = screen(&mut m).join("\n");
    assert!(s.contains("ctrl+enter search conversations"), "{s}");
    press(&mut m, "ctrl+j");
    assert!(the_bar(&m).done);
    assert!(sections(&m).contains(&"Conversations".to_string()));
}

/// The bar's head says what a new agent starts as, and Start an agent
/// says it too (cmdbar.go:1428-1429, :736).
#[test]
fn bar_says_what_new_agents_start_as() {
    let mut m = model(120, 40);
    press(&mut m, "ctrl+k");
    typed(&mut m, "#eff");
    let s = screen(&mut m).join("\n");
    assert!(s.contains("new agents:  ✻ Anthropic (Claude Code)"), "{s}");
    let start = bar::bar_places(&m, "")
        .into_iter()
        .find(|it| it.title == "Start an agent")
        .expect("Start an agent");
    assert!(
        start.meta.starts_with("in /src/checkout · "),
        "{}",
        start.meta
    );
}

/// With a Session open, Go to offers its views by name; going there
/// shows that view (cmdbar.go:783-793).
#[test]
fn bar_offers_the_sessions_views() {
    let mut m = model(120, 40);
    let key = "default/d4".to_string();
    let mut v = crate::session::SessionView::new(key.clone(), crate::session::Source::Hosted, 1);
    v.read = true;
    m.sel = key.clone();
    m.session = Some(v);
    press(&mut m, "ctrl+k");
    typed(&mut m, "session ch");
    let it = the_bar(&m)
        .items
        .iter()
        .find(|it| it.title == "Session › Changes")
        .expect("Session › Changes")
        .clone();
    assert_eq!(it.meta, "set up caching for CI");
    bar::close(&mut m);
    bar::run(&mut m, &it.go);
    let at = crate::screens::session_core::views::views(&m, &key)
        .iter()
        .position(|v| *v == "changes")
        .unwrap();
    assert_eq!(crate::screens::session_core::conn(&mut m, &key).vs.view, at);
}

/// The Session beside the list, the list having the keys, has its views
/// offered too (Go's m.host stays it), and going to one gives it the keys
/// (m.preview, m.paneFocus = true; cmdbar.go:783-793).
#[test]
fn bar_views_of_the_session_beside_the_list() {
    let mut m = model(120, 40);
    let key = "default/d4".to_string();
    let mut v = crate::session::SessionView::new(key.clone(), crate::session::Source::Hosted, 1);
    v.read = true;
    m.sel = key.clone();
    m.pane = Some(v);
    press(&mut m, "ctrl+k");
    typed(&mut m, "session ch");
    let it = the_bar(&m)
        .items
        .iter()
        .find(|it| it.title == "Session › Changes")
        .expect("Session › Changes for the pane")
        .clone();
    bar::close(&mut m);
    bar::run(&mut m, &it.go);
    assert_eq!(
        m.session.as_ref().map(|v| v.key.as_str()),
        Some(key.as_str()),
        "not given the keys"
    );
    let at = crate::screens::session_core::views::views(&m, &key)
        .iter()
        .position(|v| *v == "changes")
        .unwrap();
    assert_eq!(crate::screens::session_core::conn(&mut m, &key).vs.view, at);
}

/// ctrl+] a and ctrl+] o with a session rush doesn't run open: Go has no
/// m.host then, so the Prompt keeps its text and says why
/// (keybind.go:333).
#[test]
fn chord_refuses_an_agent_outside_rush() {
    let mut m = model(160, 40);
    let key = m.agents[3].key.clone();
    m.open(3);
    for action in ["prompt.append", "prompt.replace"] {
        m.input.text = "keep me".chars().collect();
        super::keybind::run_action(&mut m, action);
        assert_eq!(m.input.string(), "keep me", "{action}: the Prompt lost it");
        let f = m.flash.clone().expect("a flash").0;
        assert_eq!(f, "no agent open to send it to", "{action}");
        assert!(
            crate::screens::session_core::conn(&mut m, &key)
                .input
                .is_empty(),
            "{action}: moved to a box never sent"
        );
    }
}
