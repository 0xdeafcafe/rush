//! plugin_ui against Go: testdata/*.txt are what Go's sheets and file
//! editor draw of the fixtures below, plain and with trailing blanks
//! trimmed, sets split by "=====", written by a throwaway Go test at the
//! widths below. Then the keys, through a fake disk.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use rush_core::adapter::{Marketplace, Plugin as AgentPlugin, PluginParts};
use rush_core::text::Line;
use rush_plugin::{Manifest, Offer, Pick, PickAction, PickItem, Plugin, UiDo};

use super::approval::ApprovalSheet;
use super::layout::LayoutSheet;
use super::offer::OfferSheet;
use super::pick::PickSheet;
use super::picks::{OptPick, OptionalSheet};
use super::sheet::{DISCOVER, INSTALLED, MARKETS, PluginSheet};
use super::*;
use crate::frame::{Press, Sheet};

fn plain_rows(ls: Vec<Line>) -> Vec<String> {
    ls.into_iter()
        .map(|l| l.plain().trim_end_matches(' ').to_string())
        .collect()
}

fn same(sets: Vec<Vec<Line>>, want: &str, what: &str) {
    same_by(sets, want, what, |v| v);
}

/// Without the bullet that names the plugin's data folder, which is under
/// the home each test run has.
fn no_data_dir(rows: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for r in rows {
        if r.starts_with("    • writes only ") {
            skip = true;
            continue;
        }
        if skip && r.starts_with("  ") && !r.starts_with("    •") && !r.trim().is_empty() {
            continue;
        }
        skip = false;
        out.push(r);
    }
    out
}

fn same_by(sets: Vec<Vec<Line>>, want: &str, what: &str, norm: fn(Vec<String>) -> Vec<String>) {
    let want: Vec<&str> = want
        .strip_suffix('\n')
        .unwrap_or(want)
        .split("\n=====\n")
        .collect();
    assert_eq!(sets.len(), want.len(), "{what}: sets");
    for (i, (got, want)) in sets.into_iter().zip(want).enumerate() {
        let got = norm(plain_rows(got));
        let want = norm(want.split('\n').map(String::from).collect());
        for (r, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "{what}: set {i} row {r}\n got: {got:#?}");
        }
        assert_eq!(got.len(), want.len(), "{what}: set {i} rows\n{got:#?}");
    }
}

fn model() -> Model {
    Model::new("test")
}

fn pick() -> Pick {
    Pick {
        id: "h".into(),
        title: "Drafts".into(),
        about: "what you set aside".into(),
        tabs: vec!["Sent".into(), "Cleared".into()],
        empty: vec!["".into(), "nothing cleared yet".into()],
        items: vec![
            PickItem {
                id: "1".into(),
                text: "fix the login\nit fails on expired tokens\nand on refresh\nmore".into(),
                meta: "2m ago".into(),
                ..PickItem::default()
            },
            PickItem {
                id: "2".into(),
                text: "add tests for the parser".into(),
                meta: "1h ago".into(),
                ..PickItem::default()
            },
            PickItem {
                id: "3".into(),
                text: "a very long draft that goes on and on and on past the width of the sheet for sure, really it does go on".into(),
                meta: "yesterday".into(),
                ..PickItem::default()
            },
        ],
        actions: vec![
            PickAction {
                key: "enter".into(),
                name: "put back".into(),
                stay: false,
            },
            PickAction {
                key: "ctrl+d".into(),
                name: "forget".into(),
                stay: true,
            },
        ],
        ..Pick::default()
    }
}

#[test]
fn pick_matches_go() {
    let m = model();
    let mut sets = Vec::new();
    for w in [108, 60] {
        let mut s = PickSheet::new("haven", pick());
        sets.push(s.body(&m, w, 30));
        s.cur = 2;
        sets.push(s.body(&m, w, 30));
        s.tab = 1;
        sets.push(s.body(&m, w, 30));
        s.tab = 0;
        s.filter = "zzz".chars().collect();
        sets.push(s.body(&m, w, 30));
    }
    same(sets, include_str!("testdata/pick.txt"), "pick");
}

fn offer() -> Offer {
    let mut o = Offer::default();
    o.name = "weather".into();
    o.description = "Shows the weather where you are.".into();
    o.version = "1.2.0".into();
    o.dir = "/src/acme/plugins/weather".into();
    o.project = "/src/acme".into();
    o
}

#[test]
fn offer_matches_go() {
    let m = model();
    let mut sets = Vec::new();
    for w in [96, 60] {
        let mut s = OfferSheet::new(offer());
        sets.push(s.body(&m, w, 30));
        let mut o2 = offer();
        o2.update = true;
        o2.was = "0123456789abcdef".into();
        o2.commit = "fedcba9876543210".into();
        o2.commits = 3;
        let mut s = OfferSheet::new(o2);
        s.cur = 0;
        s.err = "install.sh failed: exit 1".into();
        sets.push(s.body(&m, w, 30));
        s.busy = true;
        sets.push(s.body(&m, w, 30));
    }
    same(sets, include_str!("testdata/offer.txt"), "offer");
}

fn weather() -> Plugin {
    Plugin {
        dir: "/opt/plugins/weather".into(),
        manifest: Manifest {
            name: "weather".into(),
            description: "Shows the weather.".into(),
            version: "1.2.0".into(),
            command: vec!["./weather".into()],
            network: vec!["api.weather.example:443".into()],
            ..Manifest::default()
        },
        ..Plugin::default()
    }
}

#[test]
fn approval_matches_go() {
    let m = model();
    let mut sets = Vec::new();
    for w in [96, 60] {
        let mut s = ApprovalSheet {
            p: weather(),
            rest: Vec::new(),
            cur: 0,
            busy: false,
            err: String::new(),
            changed: false,
        };
        sets.push(s.body(&m, w, 30));
        s.changed = true;
        s.cur = 1;
        s.err = "couldn't write approvals".into();
        sets.push(s.body(&m, w, 30));
    }
    same_by(
        sets,
        include_str!("testdata/approval.txt"),
        "approval",
        no_data_dir,
    );
}

fn opt(name: &str, about: &str, category: &str, on: bool) -> OptPick {
    OptPick {
        name: name.into(),
        about: about.into(),
        category: category.into(),
        on,
    }
}

#[test]
fn optional_matches_go() {
    let m = model();
    let picks = vec![
        opt(
            "drafts",
            "Keeps what you clear or send, so nothing typed is lost.",
            "Messages",
            false,
        ),
        opt(
            "queue",
            "Holds messages until the agent is free, then sends them in order, one at a time, each when the last is answered.",
            "Messages",
            true,
        ),
        opt("yap", "Talk instead of typing.", "Voice", false),
    ];
    let mut sets = Vec::new();
    for w in [96, 60] {
        let mut s = OptionalSheet {
            picks: picks.clone(),
            first: true,
            never: true,
            cur: 0,
            busy: false,
        };
        sets.push(s.body(&m, w, 30));
        s.first = false;
        s.cur = 1;
        sets.push(s.body(&m, w, 30));
        s.never = false;
        s.busy = true;
        sets.push(s.body(&m, w, 30));
    }
    same(sets, include_str!("testdata/optional.txt"), "optional");
}

#[test]
fn layout_matches_go() {
    let m = model();
    let mut sets = Vec::new();
    for w in [72, 50] {
        let mut v = LayoutSheet::default();
        sets.push(v.body(&m, w, 30));
        v.cur = 2;
        sets.push(v.body(&m, w, 30));
    }
    same(sets, include_str!("testdata/layout.txt"), "layout");
}

fn ap(id: &str, name: &str, market: &str) -> AgentPlugin {
    AgentPlugin {
        id: id.into(),
        name: name.into(),
        marketplace: market.into(),
        ..AgentPlugin::default()
    }
}

fn plugins_sheet() -> PluginSheet {
    let ad = rush_core::adapter::global()
        .all()
        .first()
        .cloned()
        .unwrap_or_else(|| Arc::new(NoAgent));
    let mut p = PluginSheet::new("k".into(), ad, "work".into(), "/work/proj".into());
    p.loaded = true;
    p.costs.insert("github@official".into(), "~1.2k tok".into());
    let mut gh = ap("github@official", "github", "official");
    gh.version = "2.1.0".into();
    gh.enabled = true;
    gh.scope = "user".into();
    gh.description = "Work with GitHub issues and pull requests.".into();
    gh.parts = PluginParts {
        skills: vec!["pr-review".into()],
        commands: vec!["pr".into(), "issue".into()],
        mcp: vec!["github".into()],
        ..PluginParts::default()
    };
    let mut notes = ap("notes@acme", "notes", "acme");
    notes.version = "0.3".into();
    notes.scope = "project".into();
    notes.project_path = "/work/proj".into();
    notes.description = "Project notes.".into();
    let mut synced = ap("synced@claude.ai", "synced", "claude.ai");
    synced.scope = "claude.ai".into();
    synced.enabled = true;
    p.installed = vec![gh, notes, synced];
    let mut linear = ap("linear@official", "linear", "official");
    linear.installs = 12345;
    linear.description = "Linear issues, from the session.".into();
    let mut slack = ap("slack@official", "slack", "official");
    slack.installs = 2_500_000;
    slack.description = "Read and post in Slack channels\nacross workspaces.".into();
    let mut tiny = ap("tiny@acme", "tiny", "acme");
    tiny.installs = 12;
    p.available = vec![linear, slack, tiny];
    p.markets = vec![
        Marketplace {
            name: "official".into(),
            source: "github".into(),
            repo: "anthropics/claude-plugins".into(),
            ..Marketplace::default()
        },
        Marketplace {
            name: "acme".into(),
            source: "directory".into(),
            location: "/src/acme-market".into(),
            ..Marketplace::default()
        },
    ];
    p
}

struct NoAgent;
impl rush_core::adapter::Adapter for NoAgent {
    fn kind(&self) -> rush_core::adapter::Kind {
        rush_core::adapter::Kind::from("none")
    }
    fn name(&self) -> String {
        "None".into()
    }
    fn features(&self) -> rush_core::adapter::Features {
        Default::default()
    }
    fn level(&self) -> rush_core::adapter::Level {
        Default::default()
    }
    fn profiles(&self) -> Vec<rush_core::adapter::Profile> {
        Vec::new()
    }
}

#[test]
fn plugins_matches_go() {
    let m = model();
    let mut sets = Vec::new();
    for w in [108, 70] {
        let mut p = plugins_sheet();
        sets.push(p.body(&m, w, 34));
        p.cur[INSTALLED] = 1;
        p.done = "turning on notes".into();
        p.changed = true;
        sets.push(p.body(&m, w, 34));
        p.tab = DISCOVER;
        p.done.clear();
        p.changed = false;
        sets.push(p.body(&m, w, 34));
        p.query = "sl".chars().collect();
        p.query_pos = 2;
        sets.push(p.body(&m, w, 34));
        p.tab = MARKETS;
        sets.push(p.body(&m, w, 34));
        p.adding = true;
        p.source = "owner/re".chars().collect();
        p.source_pos = 8;
        sets.push(p.body(&m, w, 34));
        p.adding = false;
        p.busy = "updating acme…".into();
        sets.push(p.body(&m, w, 34));
        p.busy.clear();
        p.err = "claude plugin marketplace update failed: no network".into();
        sets.push(p.body(&m, w, 34));
        let mut q = plugins_sheet();
        q.loaded = false;
        sets.push(q.body(&m, w, 34));
    }
    same(sets, include_str!("testdata/plugins.txt"), "plugins");
}

const MD_TEXT: &str = "---\nname: note\ndescription: what this is\ntype: opinion\n---\n\n# Notes\n\nSome **bold** and *italic* and `code`, with a [link](missing.md) and [[other]].\n\n- one\n- [x] done item\n- [ ] open item that is long enough to wrap around the edge of a narrow editor\n1. first\n> a quote\n\n```go\nfunc main() {\n\tx := \"str\" // c\n}\n```\n| a | b |\n---\nend";

const JS_TEXT: &str =
    "{\n  \"a\": 1,\n  \"b\": [true, null, \"x\"],\n  \"c\": {\"d\": 2.5e3},\n  \"a\": 3\n}\n";

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rush-plugin-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn doc_matches_go() {
    let dir = temp_dir("doc");
    let md = dir.join("memory").join("note.md");
    std::fs::create_dir_all(md.parent().unwrap()).unwrap();
    std::fs::write(&md, MD_TEXT).unwrap();
    let js = dir.join("settings.json");
    std::fs::write(&js, JS_TEXT).unwrap();
    let mut sets = Vec::new();
    for p in [&md, &js] {
        for w in [60, 34] {
            let mut e = doc::DocEditor::open_now(&p.to_string_lossy());
            e.view(w, 14, true);
            e.pos = e.buf.len() / 2;
            sets.push(e.view(w, 14, true));
            e.pos = e.buf.len();
            sets.push(e.view(w, 14, false));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    same(sets, include_str!("testdata/doc.txt"), "doc");
}

/// A disk that keeps what's done to it, for the keys.
#[derive(Default)]
struct Fake {
    did: Mutex<Vec<String>>,
    pending: Vec<Plugin>,
}

impl Fake {
    fn said(&self, s: String) {
        self.did.lock().unwrap().push(s);
    }
}

impl Disk for Fake {
    fn bundles_on(&self) -> BTreeMap<String, bool> {
        BTreeMap::new()
    }
    fn set_bundled(&self, name: &str, on: bool) -> Result<(), String> {
        self.said(format!("bundled {name} {on}"));
        Ok(())
    }
    fn reload(&self) -> Result<(), String> {
        self.said("reload".into());
        Ok(())
    }
    fn pending(&self) -> Vec<Plugin> {
        self.pending.clone()
    }
    fn approved_before(&self, _name: &str) -> bool {
        true
    }
    fn approve(&self, p: &Plugin) -> Result<(), String> {
        self.said(format!("approve {}", p.name));
        Ok(())
    }
    fn decline(&self, p: &Plugin) -> Result<(), String> {
        self.said(format!("decline {}", p.name));
        Ok(())
    }
    fn install(&self, o: &Offer) -> Result<(), String> {
        self.said(format!("install {}", o.name));
        Ok(())
    }
    fn skip(&self, o: &Offer) -> Result<(), String> {
        self.said(format!("skip {}", o.name));
        Ok(())
    }
    fn onboarding(&self) -> (Vec<String>, Vec<String>) {
        (Vec::new(), Vec::new())
    }
    fn mark_asked(&self, names: &[String]) {
        self.said(format!("asked {}", names.join(",")));
    }
    fn set_view(&self, v: &str) {
        self.said(format!("view {v}"));
    }
    fn offline(&self) -> bool {
        false
    }
}

fn faked(f: Fake) -> (Model, Arc<Fake>) {
    let mut m = model();
    let f = Arc::new(f);
    m.plugins().disk = f.clone();
    (m, f)
}

fn press(m: &mut Model, keys: &[&str]) {
    for k in keys {
        let p = Press::of(k).unwrap_or_else(|| panic!("no key {k}"));
        m.press_key(&p);
    }
}

fn did(f: &Fake) -> Vec<String> {
    f.did.lock().unwrap().clone()
}

// A plugin's pick: tabs, a filter, an action that keeps it open without
// the row, and one that closes it (pluginbox_test.go TestPluginPick).
#[test]
fn plugin_pick_keys() {
    let (mut m, _) = faked(Fake::default());
    let mut p = pick();
    p.items[2].tab = 1;
    hooks::plugin_do(
        &mut m,
        UiDo {
            plugin: "drafts".into(),
            kind: "pick".into(),
            pick: Some(p),
            ..UiDo::default()
        },
    );
    assert!(m.sheet_as::<PickSheet>().is_some(), "no pick shown");
    press(&mut m, &["t", "e", "s", "t"]);
    let s = m.sheet_as::<PickSheet>().unwrap();
    let shown: Vec<String> = s.shown().iter().map(|i| i.id.clone()).collect();
    assert_eq!(shown, ["2"]);
    press(&mut m, &["ctrl+d"]);
    let s = m.sheet_as::<PickSheet>().expect("forget keeps the sheet");
    assert!(s.shown().is_empty());
    press(
        &mut m,
        &["backspace", "backspace", "backspace", "backspace", "]"],
    );
    let s = m.sheet_as::<PickSheet>().unwrap();
    assert_eq!((s.tab, s.shown().len()), (1, 1));
    press(&mut m, &["enter"]);
    assert!(m.sheet.is_none(), "enter closes it");
}

#[test]
fn approval_keys() {
    let other = Plugin {
        manifest: Manifest {
            name: "other".into(),
            ..Manifest::default()
        },
        ..Plugin::default()
    };
    let (mut m, f) = faked(Fake {
        pending: vec![weather(), other],
        ..Fake::default()
    });
    approval::check(&mut m);
    let s = m.sheet_as::<ApprovalSheet>().expect("offered");
    assert!(s.changed, "approved before, so changed since");
    assert_eq!(s.p.name, "weather");
    press(&mut m, &["y"]);
    assert_eq!(did(&f), ["approve weather", "reload"]);
    let s = m.sheet_as::<ApprovalSheet>().expect("the next one");
    assert_eq!(s.p.name, "other");
    press(&mut m, &["esc"]);
    assert_eq!(did(&f), ["approve weather", "reload", "decline other"]);
    assert!(m.sheet.is_none());
}

#[test]
fn offer_keys() {
    let (mut m, f) = faked(Fake::default());
    m.plugins().offers = vec![offer()];
    offer::open(&mut m);
    // It opens on Not now: an enter meant for the list mustn't install.
    assert_eq!(m.sheet_as::<OfferSheet>().unwrap().cur, 1);
    press(&mut m, &["enter"]);
    assert_eq!(did(&f), ["skip weather"]);
    assert!(m.sheet.is_none() && m.plugins().offers.is_empty());
}

#[test]
fn optional_keys() {
    let (mut m, f) = faked(Fake::default());
    m.plugins().opt_picks = Some(vec![
        opt("drafts", "a", "Messages", false),
        opt("yap", "b", "Voice", false),
    ]);
    picks::open(&mut m);
    press(&mut m, &["down", "space", "enter"]);
    assert_eq!(did(&f), ["asked drafts,yap", "bundled yap true", "reload"]);
    assert!(m.sheet.is_none());
}

#[test]
fn layout_keys() {
    let (mut m, f) = faked(Fake::default());
    layout::ask(&mut m);
    press(&mut m, &["up"]);
    assert_eq!(m.sheet_as::<LayoutSheet>().unwrap().cur, 2);
    press(&mut m, &["2"]);
    assert_eq!(did(&f), ["view agent"]);
    assert!(m.sheet.is_none());
}

#[test]
fn notify_and_box() {
    let (mut m, _) = faked(Fake::default());
    hooks::plugin_do(
        &mut m,
        UiDo {
            plugin: "haven".into(),
            kind: "notify".into(),
            text: "saved\x07 it".into(),
            ..UiDo::default()
        },
    );
    let (said, _, bad) = m.flash.clone().unwrap();
    assert!(said.starts_with("haven: saved") && !bad, "{said}");
    // A plugin sets the Agents box only while it holds what it expects.
    m.ext.agents_box = true;
    m.input.set("typed");
    let set = |m: &mut Model, if_text: &str| {
        hooks::plugin_do(
            m,
            UiDo {
                plugin: "drafts".into(),
                kind: "input.set".into(),
                input_box: Some(rush_plugin::TextBox {
                    text: "put back".into(),
                    cursor: 3,
                    ..Default::default()
                }),
                if_text: Some(if_text.into()),
                ..UiDo::default()
            },
        );
    };
    set(&mut m, "not what's there");
    assert_eq!(m.input.string(), "typed");
    set(&mut m, "typed");
    assert_eq!((m.input.string().as_str(), m.input.pos), ("put back", 3));
}

#[test]
fn hook_states() {
    assert_eq!(
        hooks::change_kind("working", "done"),
        Some(rush_plugin::EV_TURN_ENDED)
    );
    assert_eq!(
        hooks::change_kind("blocked", "working"),
        Some(rush_plugin::EV_TURN_ENDED)
    );
    assert_eq!(hooks::change_kind("done", "idle"), None);
    assert_eq!(
        hooks::change_kind("done", "working"),
        Some(rush_plugin::EV_TURN_STARTED)
    );
    assert_eq!(
        hooks::change_kind("done", "halted"),
        Some(rush_plugin::EV_SESSION_STOPPED)
    );
    assert_eq!(installs(12345), "12.3k");
    assert_eq!(installs(2_500_000), "2.5M");
    assert_eq!(installs(12), "12");
}

#[test]
fn embed_keys() {
    let b = |s: &str| embed::key_bytes(&Press::of(s).unwrap());
    assert_eq!(b("enter"), b"\r");
    assert_eq!(b("ctrl+a"), [1]);
    assert_eq!(b("alt+x"), b"\x1bx");
    assert_eq!(b("super+left"), [1]);
    assert_eq!(b("q"), b"q");
    assert_eq!(embed::paste_bytes("hi"), b"\x1b[200~hi\x1b[201~");
}

#[test]
fn sidebar_places() {
    use rush_plugin::{Sidebar, SidebarAgent, SidebarSection};
    let mut sb = Sidebar {
        plugin: "haven".into(),
        title: "Haven".into(),
        sections: vec![
            SidebarSection {
                title: "Now".into(),
                ..SidebarSection::default()
            },
            SidebarSection {
                title: "Later".into(),
                ..SidebarSection::default()
            },
        ],
        ..Sidebar::default()
    };
    sb.agents = Some(BTreeMap::from([(
        "s1".to_string(),
        SidebarAgent {
            name: "fixer".into(),
            section: "Later".into(),
            order: 4,
        },
    )]));
    let mut a = rush_core::agent::Agent::default();
    a.job.session_id = "s1".into();
    a.key = "k1".into();
    assert_eq!(sidebar::place(&sb, &a), ("Later".to_string(), 1, 4));
    let b = rush_core::agent::Agent::default();
    assert_eq!(sidebar::place(&sb, &b), ("Other".to_string(), 2, 0));
    let sbs = vec![sb.clone()];
    assert_eq!(sidebar::group_modes(&sbs).last().unwrap(), "plugin:haven");
    assert_eq!(sidebar::group_label(&sbs, "plugin:haven"), "Haven");
    let names = sidebar::agent_names(&sb, std::slice::from_ref(&a), &Default::default());
    assert_eq!(names.get("k1").map(String::as_str), Some("fixer"));
}

#[test]
fn doc_editing() {
    let dir = temp_dir("edit");
    // A list goes on with enter, and an empty item ends it.
    let path = dir.join("memory").join("note.md");
    let mut e = doc::DocEditor::open_now(&path.to_string_lossy());
    e.view(60, 10, true);
    assert!(e.missing && e.kind == doc::Kind::Markdown);
    type_in(&mut e, "# Notes\n- one\ntwo\n\n\nafter");
    assert_eq!(e.text(), "# Notes\n- one\n- two\n\nafter");
    // Numbers count up; a task box carries on unticked; ctrl+t ticks it.
    let mut e2 = doc::DocEditor::open_now(&dir.join("x.md").to_string_lossy());
    type_in(&mut e2, "1. a\nb\n\n- [x] done\nnext");
    assert_eq!(e2.text(), "1. a\n2. b\n- [x] done\n- [ ] next");
    key(&mut e2, "ctrl+t");
    assert!(e2.text().ends_with("- [x] next"), "{}", e2.text());
    key(&mut e2, "ctrl+z");
    key(&mut e2, "ctrl+z");
    assert!(!e2.text().contains("next"), "{}", e2.text());
    // A memory note is checked for what it needs.
    let d = e.problems().to_vec();
    assert!(d.len() == 1 && d[0].msg.contains("no frontmatter"), "{d:?}");
    e.change(
        "---\nname: n\ndescription: d\ntype: opinion\n---\n\nSee [x](missing.md).\n```go\nx := 1\n"
            .chars()
            .collect(),
        0,
        false,
    );
    let got: Vec<String> = e
        .problems()
        .iter()
        .map(|d| format!("{}:{}", d.line + 1, d.msg))
        .collect();
    let got = got.join("\n");
    for w in [
        "7:missing.md doesn't exist",
        "8:this code block never closes",
        "4:type is one of",
    ] {
        assert!(got.contains(w), "missing {w}:\n{got}");
    }
    e.save().unwrap();
    assert!(!e.dirty());
    let b = std::fs::read_to_string(&path).unwrap();
    assert!(b.starts_with("---\nname: n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn doc_json() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("json");
    let path = dir.join("settings.json");
    std::fs::write(&path, "{\n\t\"a\": 1\n}\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut e = doc::DocEditor::open_now(&path.to_string_lossy());
    e.view(60, 10, true);
    assert_eq!(e.indent, "\t");
    assert!(e.problems().is_empty());
    // Brackets and quotes pair up, and enter inside a pair opens it.
    e.change(Vec::new(), 0, false);
    type_in(&mut e, "{\"b");
    assert_eq!(e.text(), "{\"b\"}");
    key(&mut e, "\""); // steps over the closing quote
    type_in(&mut e, ": [");
    key(&mut e, "enter");
    assert_eq!(e.text(), "{\"b\": [\n\t\n]}");
    // A comma before the close is found, where it is.
    e.change("{\n\t\"a\": 1,\n}".chars().collect(), 0, false);
    let d = e.problems().to_vec();
    assert!(
        d.len() == 1 && d[0].line == 1 && d[0].msg.contains("comma"),
        "{d:?}"
    );
    e.change(r#"{"a": ["x",]}"#.chars().collect(), 0, false);
    let d = e.problems().to_vec();
    assert!(
        d.len() == 1 && d[0].msg.contains("comma") && d[0].col == 10,
        "{d:?}"
    );
    e.change(r#"{"a":1,"a":2}"#.chars().collect(), 0, false);
    let d = e.problems().to_vec();
    assert!(
        d.len() == 1 && d[0].msg.contains(r#""a" is in this object twice"#),
        "{d:?}"
    );
    // ctrl+t lays it out; saving keeps the file's permissions.
    e.change(r#"{"b":[1,{}],"a":"<&>"}"#.chars().collect(), 0, false);
    assert_eq!(e.key(&Press::of("ctrl+t").unwrap()).0, "format");
    let out = doc::format_json(&e.text(), &e.indent).unwrap();
    assert_eq!(
        out,
        "{\n\t\"b\": [\n\t\t1,\n\t\t{}\n\t],\n\t\"a\": \"<&>\"\n}\n"
    );
    e.save().unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn json_messages() {
    let d = doc::json_problems("{\"a\": 1");
    assert_eq!(d[0].msg, "it ends before every { [ or \" is closed");
    let d = doc::json_problems("{\"a\": 1} 2");
    assert_eq!(d[0].msg, "more after the JSON ends: a file holds one value");
    assert_eq!((d[0].line, d[0].col), (0, 9));
    let d = doc::json_problems("{\"a\": {\"b\": x}}");
    assert_eq!(
        d[0].msg,
        "invalid character 'x' at start of value · in /a/b"
    );
}

fn key(e: &mut doc::DocEditor, s: &str) {
    e.key(&Press::of(s).unwrap_or_else(|| panic!("no key {s}")));
}

fn type_in(e: &mut doc::DocEditor, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => key(e, "enter"),
            ' ' => key(e, "space"),
            c => key(e, &c.to_string()),
        }
    }
}

#[test]
fn artifacts_found() {
    assert_eq!(
        artifacts::url_in("Published (Version 2): https://claude.ai/a/x1. Done"),
        "https://claude.ai/a/x1"
    );
    assert_eq!(artifacts::url_in("no link http: here"), "");
    let arts = vec![artifacts::Artifact {
        url: "https://claude.ai/a/x1".into(),
        file: "/tmp/report.html".into(),
        about: "The report".into(),
        version: 2,
        versions: 2,
        turn: 3,
        at: 1_000,
    }];
    let rows = artifacts::artifact_lines(&arts, 60, 61_000, "art:https://claude.ai/a/x1", true);
    let text: Vec<String> = rows.iter().map(|(l, _)| l.plain()).collect();
    assert!(text[0].starts_with("  Artifacts  1   pages this session published"));
    assert!(
        text[2].starts_with("▍ ◆ report") && text[2].trim_end().ends_with("v2 · #3 · 1m ago"),
        "{text:#?}"
    );
    assert_eq!(rows[2].1, "art:https://claude.ai/a/x1");
    let _ = HashSet::<String>::new();
}
