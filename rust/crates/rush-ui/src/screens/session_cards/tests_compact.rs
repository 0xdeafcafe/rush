//! #compact's tests, after Go's compactsheet_test.go and
//! compacttyped_test.go, with adapters of the tests' own in the registry.

use rush_core::adapter::{
    Adapter, Answer, Feature, Features, Kind, Level, Profile, Querier, Query, Registry, Support,
};
use rush_core::text::Line;

use super::compact::*;
use super::tests::*;
use super::*;
use crate::frame::{Press, Sheet};

/// An adapter that has the features given and, with quick, a quick model.
struct Fake {
    kind: &'static str,
    has: Vec<Feature>,
    quick: &'static str,
}

impl Adapter for Fake {
    fn kind(&self) -> Kind {
        Kind::from(self.kind)
    }
    fn name(&self) -> String {
        self.kind.to_string()
    }
    fn features(&self) -> Features {
        self.has.iter().map(|&f| (f, Support::YES)).collect()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn querier(&self) -> Option<&dyn Querier> {
        (!self.quick.is_empty()).then_some(self as &dyn Querier)
    }
}

impl Querier for Fake {
    fn query_command(&self, _: &Profile, _: &Query) -> (Vec<String>, Vec<String>) {
        (Vec::new(), Vec::new())
    }
    fn query_answer(&self, _: &[u8]) -> Option<Answer> {
        None
    }
    fn query_models(&self) -> (String, String) {
        (self.quick.into(), String::new())
    }
}

/// A hosted model whose registry holds claude (compacts, rewinds, has a
/// quick model) and codex (compacts only).
fn compact_model(kind: &str, proto: i32, state: &str) -> Model {
    let mut m = hosted(120, 40);
    m.reg = Registry::new();
    m.reg.register(Fake {
        kind: "claude",
        has: vec![Feature::Compact, Feature::Rewind],
        quick: "haiku",
    });
    for k in ["codex", "ollama-codex"] {
        m.reg.register(Fake {
            kind: k,
            has: vec![Feature::Compact],
            quick: "",
        });
    }
    set_info(&mut m, |i| {
        i.kind = kind.into();
        i.proto = proto;
        i.state = state.into();
    });
    m
}

fn key(s: &mut CompactSheet, m: &mut Model, k: &str) -> Out {
    s.key(m, &Press::of(k).unwrap())
}

fn flat(ls: &[Line]) -> String {
    text(ls).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// TestOldHostKeepsNativeCompactionButBlocksExternal.
#[test]
fn old_host_keeps_native_compaction_but_blocks_external() {
    let mut m = compact_model("claude", 8, "idle");
    open_compact(&mut m, "k");
    let s = m.sheet_as::<CompactSheet>().expect("no compaction picker");
    assert!(!s.loading, "an old host doesn't look for other models");
    assert_eq!(s.opts.len(), 1);
    assert!(s.opts[0].model.is_empty(), "native option lost");
    let mut s = std::mem::take(s);
    let body = flat(&s.body(&m, 100, 30));
    assert!(
        body.contains("Restart") && body.contains("native compaction remains available"),
        "{body}"
    );
    let o = Summ {
        kind: "ollama".into(),
        model: "writer".into(),
        ..Summ::default()
    };
    compact_by(&mut m, "k", o, "");
    assert!(acts(&mut m).is_empty(), "unguarded external compaction");
}

/// TestCompactionDefaultIsExplicitAndPersistent: d saves the one under
/// the cursor and keeps the sheet; native puts the default back.
#[test]
fn compaction_default_is_explicit_and_persistent() {
    let mut m = compact_model("claude", 9, "idle");
    let mut s = CompactSheet {
        opts: vec![
            Summ {
                label: "native".into(),
                ..Summ::default()
            },
            Summ {
                kind: "ollama".into(),
                model: "writer".into(),
                label: "Ollama".into(),
                prune: false,
            },
        ],
        cur: 1,
        ..CompactSheet::default()
    };
    assert_eq!(key(&mut s, &mut m, "d"), Out::Draw, "d keeps the sheet");
    assert!(acts(&mut m).is_empty(), "saving a default compacts nothing");
    let saved = |m: &mut Model, k: &str| m.kept::<super::tests::Fake>().settings[k].clone();
    assert_eq!(saved(&mut m, "compactKind"), "ollama");
    assert_eq!(saved(&mut m, "compactModel"), "writer");
    s.cur = 0;
    key(&mut s, &mut m, "d");
    assert_eq!(saved(&mut m, "compactKind"), "");
    assert_eq!(saved(&mut m, "compactModel"), "");
}

/// TestEmptyCompactionPickerCannotPanic.
#[test]
fn empty_compaction_picker_cannot_panic() {
    let mut m = compact_model("claude", 9, "idle");
    let mut s = CompactSheet::default();
    for k in ["down", "enter", "d", "up", "enter", "pgdown", "pgup"] {
        key(&mut s, &mut m, k);
        s.body(&m, 50, 16);
    }
}

/// TestCompactionExplanationsStayInsideSheetAndScroll.
#[test]
fn compaction_explanations_stay_inside_sheet_and_scroll() {
    let mut m = compact_model("claude", 9, "idle");
    let mut s = CompactSheet {
        opts: vec![Summ {
            label: "native".into(),
            ..Summ::default()
        }],
        errs: vec![format!(
            "{}LAST EXPLANATION",
            "Classifier explanation details. ".repeat(30)
        )],
        ..CompactSheet::default()
    };
    let first = s.body(&m, 50, 16);
    assert!(first.len() <= 16, "height {}", first.len());
    for l in &first {
        assert!(l.width() <= 50, "too wide: {}", l.plain());
    }
    for _ in 0..30 {
        key(&mut s, &mut m, "pgdown");
        s.body(&m, 50, 16);
    }
    let last = text(&s.body(&m, 50, 16));
    assert!(last.contains("LAST EXPLANATION"), "{last}");
}

/// TestCompactionUsesAuthoritativeHostKind: the host's kind, not the
/// list's, says what the session can do.
#[test]
fn compaction_uses_authoritative_host_kind() {
    let mut m = compact_model("unknown-no-compact", 9, "idle");
    open_compact(&mut m, "k");
    assert!(m.sheet_as::<CompactSheet>().unwrap().opts.is_empty());
}

/// TestCodexOffersNativeCompactionWithoutExternalSummaries.
#[test]
fn codex_offers_native_compaction_without_external_summaries() {
    for kind in ["codex", "ollama-codex"] {
        let mut m = compact_model(kind, 10, "idle");
        open_compact(&mut m, "k");
        let s = m.sheet_as::<CompactSheet>().unwrap();
        assert_eq!(s.opts.len(), 1, "{kind}");
        assert!(s.opts[0].model.is_empty() && !s.loading, "{kind}");
    }
}

/// TestTypedCompactTakesTheFastPath: a typed /compact on a cold cache is
/// summarised by the quick model when it safely can, and is the harness's
/// own otherwise.
#[test]
fn typed_compact_takes_the_fast_path() {
    const HOUR: i64 = 3_600_000;
    for (name, kind, state, arg, proto, warm, fast, handled) in [
        (
            "idle Claude, cold",
            "claude",
            "idle",
            "",
            9,
            false,
            true,
            true,
        ),
        (
            "idle Claude, warm",
            "claude",
            "idle",
            "",
            9,
            true,
            false,
            false,
        ),
        (
            "instructions",
            "claude",
            "idle",
            "keep the API notes",
            9,
            false,
            false,
            false,
        ),
        ("mid-turn", "claude", "working", "", 9, false, false, false),
        ("old host", "claude", "idle", "", 8, false, false, false),
        ("no fast model", "codex", "idle", "", 9, false, false, false),
        (
            "asked for native",
            "claude",
            "idle",
            "native",
            9,
            false,
            false,
            true,
        ),
    ] {
        let mut m = compact_model(kind, proto, state);
        let now = now(&m);
        let last = if warm {
            now - 5 * 60_000
        } else {
            now - 3 * HOUR
        };
        m.session.as_mut().unwrap().sess.requests = vec![rush_convo::Request {
            at: last,
            ..rush_convo::Request::default()
        }];
        let out = compact_typed(&mut m, "k", arg);
        assert_eq!(out != Out::Pass, handled, "{name}: handled");
        let compacting = m.session.as_ref().unwrap().sess.compacting().is_some();
        assert_eq!(compacting, fast, "{name}: fast path");
    }
}

/// Ollama as a summariser: one model that sums up, one it can't use and
/// says why.
struct Local;

impl Adapter for Local {
    fn kind(&self) -> Kind {
        Kind::from("ollama")
    }
    fn name(&self) -> String {
        "Ollama".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::Preview
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    fn summarizer(&self) -> Option<&dyn rush_core::adapter::Summarizer> {
        Some(self)
    }
}

impl rush_core::adapter::Summarizer for Local {
    fn summary_models(&self) -> rush_core::adapter::Result<Vec<String>> {
        Ok(vec!["qwen3:8b".into()])
    }
    fn summarize(&self, _: &str, _: &str, _: &str) -> rush_core::adapter::Result<String> {
        Ok(String::new())
    }
    fn summary_exclusions(&self) -> Vec<String> {
        vec!["tev1:4b: a decision classifier, not a writer".into()]
    }
}

/// The sheet lists Ollama's models, then its decision models to prune
/// with, and says why the classifiers can't sum up (openCompact's later:
/// SummaryExclusions, ollama.DecisionModels).
#[test]
fn compact_lists_ollamas_decision_models_to_prune_with() {
    let mut m = compact_model("claude", 9, "idle");
    m.reg.register(Local);
    let fake = crate::screens::session_core::host::Fake::new();
    *fake.deciders.lock().unwrap() = vec!["tev1:4b".into()];
    crate::screens::session_core::host::set_hosts(&mut m, std::sync::Arc::new(fake));
    open_compact(&mut m, "k");
    let mut s = std::mem::take(m.sheet_as::<CompactSheet>().unwrap());
    assert!(!s.loading);
    let ollama: Vec<&Summ> = s.opts.iter().filter(|o| o.kind == "ollama").collect();
    assert_eq!(
        ollama,
        [
            &Summ {
                kind: "ollama".into(),
                label: "Ollama".into(),
                model: "qwen3:8b".into(),
                prune: false,
            },
            &pruner("tev1:4b"),
        ]
    );
    let body = flat(&s.body(&m, 72, 30));
    assert!(
        body.contains("Ollama prune tev1:4b · experimental: keeps steps word for word"),
        "{body}"
    );
    assert!(
        body.contains("! tev1:4b: a decision classifier, not a writer"),
        "{body}"
    );
}
