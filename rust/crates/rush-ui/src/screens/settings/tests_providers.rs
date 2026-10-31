//! Settings › Providers › Ollama and a harness's own sign-in: after Go's
//! settings_ollama_models.go and settings_auth.go. Ollama is a fake one
//! on a local port (rush-ollama's testserver), and the sign-in a fake
//! adapter whose command is /usr/bin/true or /usr/bin/false: nothing here
//! signs in to anything or reads the real home.

use std::process::Command as Cmd;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    Adapter, AdapterRef, Authenticator, Features, Kind, Level, Profile, Registry,
};
use rush_ollama::Local;
use rush_ollama::testserver::serve;
use rush_state::Store;

use super::form::Setting;
use super::{config, harnesses, ollama, use_store};
use crate::frame::messages::SayKind;
use crate::model::Model;

fn model() -> (Model, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    rush_state::set_thread_dir(Some(dir.path().to_path_buf()));
    rush_state::set_thread_cache_dir(Some(dir.path().join("cache")));
    let mut m = Model::new("0.1.0");
    m.w = 140;
    m.h = 50;
    use_store(&mut m, Store::load_with_env(&|_| None));
    (m, dir)
}

fn said(m: &Model) -> (SayKind, String) {
    let l = m.said.last().expect("rush said something");
    (l.kind, l.text.clone())
}

fn labels(rows: &[Setting]) -> Vec<String> {
    rows.iter().map(|r| r.label.clone()).collect()
}

fn key(m: &mut Model, row: &Setting, s: &str) -> crate::frame::Out {
    (row.key.as_ref().expect("a row with keys"))(m, s)
}

/// An Ollama with a coder, an embedder, a decision model and one it
/// can't read; what it was asked to delete and pull, in order.
fn fake_ollama(dir: &std::path::Path) -> (Arc<Local>, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let a2 = asked.clone();
    let url = serve(move |r| {
        let out = match r.path.as_str() {
            "/api/version" => r#"{"version":"test"}"#.to_string(),
            "/api/tags" => {
                r#"{"models":[{"name":"coder"},{"name":"embed"},{"name":"tev1"},{"name":"gone"}]}"#
                    .to_string()
            }
            "/api/ps" => r#"{"models":[{"name":"coder","context_length":8192}]}"#.to_string(),
            "/api/show" => match r.json()["model"].as_str().unwrap_or("") {
                "coder" => r#"{"capabilities":["tools"],"details":{"parameter_size":"30B"},"model_info":{"x.context_length":32768}}"#.to_string(),
                "embed" => r#"{"capabilities":["embedding"]}"#.to_string(),
                "tev1" => r#"{"capabilities":["decision"],"details":{"parameter_size":"1.2B"}}"#.to_string(),
                _ => return (500, r#"{"error":"missing blob"}"#.to_string()),
            },
            "/api/delete" => {
                a2.lock().unwrap().push(format!("delete {}", r.json()["model"]));
                "{}".to_string()
            }
            "/api/pull" => {
                a2.lock().unwrap().push(format!("pull {}", r.json()["model"]));
                "{\"status\":\"pulling\",\"total\":10,\"completed\":5}\n{\"status\":\"success\"}\n"
                    .to_string()
            }
            p => panic!("nothing loads a model: {p}"),
        };
        (200, out)
    });
    let l = Local::with(
        Registry::new(),
        Some(&url),
        Some(dir.to_path_buf()),
        Some("/nonexistent/ollama"),
    );
    (l, asked)
}

/// Before a refresh only the refresh and the pull show; after one, the
/// context of the model sessions start on, each model that can't code,
/// and under Pruning each decision model (the one that prunes marked)
/// and each Ollama can't read (ollamaModelSettings, decisionSection).
#[test]
fn ollama_sections() {
    let (mut m, d) = model();
    let (l, _) = fake_ollama(d.path());
    assert_eq!(
        labels(&ollama::model_settings(&l, "")),
        ["Refresh local models"]
    );
    let p = ollama::decision_section(&m, &l);
    assert_eq!(
        (p.title.as_str(), p.note.as_str()),
        (
            "Pruning",
            "experimental · local decision models for #compact"
        )
    );
    assert_eq!(labels(&p.rows), ["Pull a decision model"]);

    // Refresh: every Ollama agent lists the models that code.
    let rows = ollama::model_settings(&l, "coder");
    assert_eq!(labels(&rows), ["Context size", "Refresh local models"]);
    assert_eq!(rows[0].value, "automatic");
    key(&mut m, &rows[1], "enter");
    assert_eq!(
        said(&m),
        (
            SayKind::Said,
            "Ollama ready · 3 installed models · 1 support coding tools".into()
        )
    );
    let listed = super::common::models(&m, "ollama-pi");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (
            listed[0].id.as_str(),
            listed[0].note.as_str(),
            listed[0].context
        ),
        ("coder", "30B · loads on demand", 32768)
    );

    let rows = ollama::model_settings(&l, "coder");
    assert_eq!(
        labels(&rows),
        ["Context size", "Refresh local models", "embed"]
    );
    assert!(
        rows[0]
            .what
            .ends_with("Currently loaded: 8k tokens. Model maximum: 32k tokens."),
        "{}",
        rows[0].what
    );
    assert_eq!(rows[2].value, "no coding tools");

    let p = ollama::decision_section(&m, &l);
    assert_eq!(labels(&p.rows), ["tev1", "gone", "Pull a decision model"]);
    assert_eq!(p.rows[0].value, "1.2B · prunes", "the first found prunes");
    assert_eq!(p.rows[1].value, "files missing");
    assert!(
        p.rows[1].what.contains("missing blob"),
        "{}",
        p.rows[1].what
    );
}

/// enter on a decision model prunes with it; x removes it once you say
/// so, and it no longer prunes; enter on one Ollama can't read pulls it
/// again (decisionSection's keys, pullModel).
#[test]
fn ollama_pruning_keys() {
    let (mut m, d) = model();
    let (l, asked) = fake_ollama(d.path());
    l.installed_models(None).expect("found");
    let p = ollama::decision_section(&m, &l);
    key(&mut m, &p.rows[0], "enter");
    assert_eq!(config(&m).prune_model, "tev1");
    assert_eq!(said(&m), (SayKind::Said, "Pruning with tev1".into()));

    key(&mut m, &p.rows[0], "x");
    assert_eq!(
        m.confirm.as_ref().map(|c| c.question.clone()).as_deref(),
        Some("Remove tev1 from Ollama?")
    );
    m.confirm_key("y");
    assert_eq!(*asked.lock().unwrap(), ["delete \"tev1\""]);
    assert_eq!(config(&m).prune_model, "", "a removed model prunes no more");
    assert_eq!(said(&m), (SayKind::Said, "Removed tev1".into()));

    // The one Ollama can't read, pulled again.
    key(&mut m, &p.rows[1], "enter");
    assert_eq!(*asked.lock().unwrap(), ["delete \"tev1\"", "pull \"gone\""]);
    // It still can't be read: discovery says so.
    assert_eq!(said(&m), (SayKind::Said, "gone is ready".into()));
}

/// A harness whose sign-in is its own program's, run as /usr/bin/true
/// or /usr/bin/false.
struct Native(&'static str);

impl Adapter for Native {
    fn kind(&self) -> Kind {
        Kind::new("native")
    }
    fn name(&self) -> String {
        "Native".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn authenticator(&self) -> Option<&dyn Authenticator> {
        Some(self)
    }
}

impl Authenticator for Native {
    fn sign_in_command(&self, _: &Profile) -> rush_core::adapter::Result<Cmd> {
        Ok(Cmd::new(self.0))
    }
}

/// Sign in hands the terminal to the harness's own sign-in; once it ends
/// well rush says so and checks again, and one that fails says why
/// (nativeSignIn).
#[test]
fn native_sign_in_runs_in_the_terminal() {
    let (mut m, _d) = model();
    let ok: AdapterRef = Arc::new(Native("/usr/bin/true"));
    harnesses::native_sign_in_with(&mut m, ok, "native");
    assert!(m.said.is_empty(), "nothing said before it runs");
    assert_eq!(m.run_outside(), 1, "the terminal handed to it");
    assert_eq!(
        said(&m),
        (
            SayKind::Said,
            "Sign-in finished · checking credentials".into()
        )
    );
    let bad: AdapterRef = Arc::new(Native("/usr/bin/false"));
    harnesses::native_sign_in_with(&mut m, bad, "native");
    assert_eq!(m.run_outside(), 1);
    assert_eq!(
        said(&m),
        (SayKind::Failed, "Sign-in failed: exit status 1".into())
    );
}

/// A look's preview of the Agents list, cut short, ends in "↓ N more
/// lines" at the height it's drawn, as the list itself does
/// (listPreview, view.go:1741).
#[test]
fn layout_preview_says_whats_below() {
    let (mut m, _d) = model();
    twelve_working(&mut m);
    super::interface::refresh_preview(&mut m);
    let secs = super::interface::sections(&m);
    let layout = super::form::flat(&secs)
        .into_iter()
        .find(|s| s.label == "Layout")
        .expect("Layout");
    let p = layout.preview.as_ref().expect("a preview");
    for h in [8, 12, 18] {
        let lines: Vec<String> = p(110, h).iter().map(|l| l.plain()).collect();
        // The box's last row is its bottom edge; the one above it, the
        // list's last.
        let last = &lines[lines.len() - 2];
        assert!(
            last.contains("more lines"),
            "h {h}: the list's last row says what's below:\n{}",
            lines.join("\n")
        );
    }
}

/// The preview is drawn at the width it's shown at (the list's, when
/// that's less), so it has that width's columns, not wider ones cut short
/// with "…" (view.go:1526 listPreview: min(listW, w)).
#[test]
fn layout_preview_is_the_lists_width() {
    let (mut m, _d) = model();
    twelve_working(&mut m);
    m.w = 160;
    super::interface::refresh_preview(&mut m);
    let secs = super::interface::sections(&m);
    let layout = super::form::flat(&secs)
        .into_iter()
        .find(|s| s.label == "Layout")
        .expect("Layout");
    let p = layout.preview.as_ref().expect("a preview");
    // Inline (the dialog's width) and in the help sheet (88 wide), each at
    // its own width: TIME stays the header's last column, never cut.
    for w in [m.w - 6, 88] {
        let lines: Vec<String> = p(w, 12).iter().map(|l| l.plain()).collect();
        assert!(
            lines[1].contains("TIME") && !lines[1].contains('…'),
            "at {w}:\n{}",
            lines.join("\n")
        );
    }
}

/// Twelve agents working in one checkout: more than a preview has room for.
fn twelve_working(m: &mut Model) {
    let agents: Vec<rush_core::agent::Agent> = (0..12)
        .map(|i| {
            let mut a = rush_core::agent::Agent {
                key: format!("default/a{i}"),
                display_name: format!("agent {i}"),
                repo: "/src/checkout".into(),
                interactive: true,
                ..Default::default()
            };
            a.job.id = format!("a{i}");
            a.job.state = rush_core::agent::State::Working;
            a.job.cwd = "/src/checkout".into();
            a
        })
        .collect();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots: vec!["/src/checkout".into(); 12],
        previews: Default::default(),
        priced: true,
    });
}

/// The screen changing size takes General's preview again at the new
/// width, as Go draws it each frame (listPreview).
#[test]
fn layout_preview_follows_the_width() {
    let (mut m, _d) = model();
    twelve_working(&mut m);
    m.w = 200;
    super::open_dialog(&mut m, super::PAGE_GENERAL);
    super::interface::refresh_preview(&mut m);
    m.w = 80;
    super::resized(&mut m);
    let secs = super::interface::sections(&m);
    let layout = super::form::flat(&secs)
        .into_iter()
        .find(|s| s.label == "Layout")
        .expect("Layout");
    let p = layout.preview.as_ref().expect("a preview");
    let lines: Vec<String> = p(74, 12).iter().map(|l| l.plain()).collect();
    // Its header ends in TIME, the list's last column, not cut to fit.
    assert!(
        lines[1].contains("TIME"),
        "taken at the new width:\n{}",
        lines.join("\n")
    );
}

/// Providers' list draws each provider's glyph in its own colour, and the
/// line picked as Go's paint(cText, label) shows a label with a painted
/// glyph in it: the glyph keeps its colour and the reset after it leaves
/// the name in the terminal's own colour (catalogList).
#[test]
fn provider_list_glyphs_keep_their_colours() {
    use rush_core::text::{Color, Line};

    use super::catalog;
    use super::form::Section;
    use crate::style::{ORANGE, TEXT, line, plain, sp};

    let blue = Color::Rgb(0x8a, 0xb4, 0xf8);
    let row = |name: &str| Setting {
        label: format!("✦ {name}"),
        painted: line([sp("✦", blue), plain(" "), plain(name)]),
        ..Setting::default()
    };
    let plain_row = Setting {
        label: "Codex".into(),
        ..Setting::default()
    };
    let secs = [Section::new(
        "Set up",
        vec![row("Google"), row("Other"), plain_row],
    )];
    let fg = |l: &Line, text: &str| -> Color {
        l.spans
            .iter()
            .find(|s| s.text.contains(text))
            .unwrap_or_else(|| panic!("no {text:?} in {l:?}"))
            .style
            .fg
    };
    for inside in [false, true] {
        let (rows, cursor, _) = catalog::list(&secs, 0, inside, 30);
        assert_eq!(cursor, 1);
        let picked = &rows[1];
        assert_eq!(fg(picked, if inside { "▸" } else { "▍" }), ORANGE);
        assert_eq!(fg(picked, "✦"), blue, "the picked line's glyph");
        assert_eq!(fg(picked, "Google"), Color::Default, "the picked name");
        assert_eq!(fg(&rows[2], "✦"), blue, "another line's glyph");
        assert_eq!(fg(&rows[2], "Other"), Color::Default);
    }
    // A label of no colours of its own is the text colour when picked.
    let (rows, _, _) = catalog::list(&secs, 2, false, 30);
    assert_eq!(fg(&rows[3], "Codex"), TEXT);
}
