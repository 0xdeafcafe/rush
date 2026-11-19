//! Proposals through the disk engine's public API: what's taken, what's
//! dropped as said already, what's refused and why; and a folder removed
//! once when a plugin's candidate is in a cache folder a rule removes.

use rush_core::time::Ms;
use serde_json::json;

use crate::disk::*;

const H: &str = "/Users/me";

fn proposal(rule: serde_json::Value, evidence: &str) -> Proposal {
    serde_json::from_value(json!({ "rule": rule, "evidence": evidence })).unwrap()
}

fn cache(id: &str, path: &str, idle: u64) -> serde_json::Value {
    json!({
        "id": id,
        "title": "a cache",
        "kind": { "type": "cache-dir", "path": path, "why": "its app makes it again" },
        "low": { "idleSecs": idle },
    })
}

// Each proposal against the built-in rules and one waiting: taken, the
// same as what's there, or refused with why.
#[test]
fn proposals_checked() {
    let waiting = proposal(
        cache(
            "cache:Library/Caches/Spotify",
            "~/Library/Caches/Spotify",
            0,
        ),
        "9G",
    );
    let ev = "~/x: 3.1G, made by x, which makes it again";
    let cases: Vec<(&str, Proposal, &str)> = vec![
        (
            "a cache folder in a cache root",
            proposal(
                cache("cache:Library/Caches/Zed", "~/Library/Caches/Zed", 0),
                ev,
            ),
            "added",
        ),
        (
            "a folder two below home",
            proposal(cache("zed-logs", "~/.zed/logs", 604800), ev),
            "added",
        ),
        (
            "the same folder waiting, by another id and words",
            proposal(cache("spotify", "/Users/me/Library/Caches/Spotify/", 0), ev),
            "same",
        ),
        (
            "a built-in, by another id",
            proposal(cache("go", "~/Library/Caches/go-build", 0), ev),
            "same",
        ),
        (
            "the same folder at other numbers",
            proposal(
                cache("spotify-week", "~/Library/Caches/Spotify", 604800),
                ev,
            ),
            "added",
        ),
        (
            "what rush never touches",
            proposal(
                cache("claude", "~/Library/Application Support/Claude", 0),
                ev,
            ),
            "isn't a folder a rule may name",
        ),
        (
            "~/.claude",
            proposal(cache("claude", "~/.claude/projects", 0), ev),
            "isn't a folder a rule may name",
        ),
        (
            "your documents",
            proposal(cache("docs", "~/Documents/old", 0), ev),
            "isn't a folder a rule may name",
        ),
        (
            "a relative path",
            proposal(cache("rel", "Library/Caches/x", 0), ev),
            "isn't a folder a rule may name",
        ),
        (
            "a path climbing out",
            proposal(cache("up", "~/Library/Caches/../Mail", 0), ev),
            "isn't a folder a rule may name",
        ),
        (
            "pnpm's store, pruned",
            proposal(
                json!({
                    "id": "pnpm-store",
                    "title": "pnpm's store, pruned",
                    "kind": { "type": "tool-command", "command": ["pnpm", "store", "prune"], "why": "pnpm's store; installs fetch it again" },
                    "low": {},
                }),
                ev,
            ),
            "added",
        ),
        (
            "pnpm's store removed whole",
            proposal(
                json!({
                    "id": "plugin:js.pnpm-store",
                    "title": "pnpm's store",
                    "kind": { "type": "plugin-candidate", "kind": "js.pnpm-store" },
                    "low": { "idleSecs": 0 },
                }),
                ev,
            ),
            "pnpm store prune, never removed whole",
        ),
        (
            "a plugin's kind",
            proposal(
                json!({
                    "id": "plugin:rust.sccache",
                    "title": "sccache's cache",
                    "kind": { "type": "plugin-candidate", "kind": "rust.sccache" },
                    "low": { "idleSecs": 604800 },
                }),
                ev,
            ),
            "added",
        ),
        (
            "a kind the engine finds itself",
            proposal(
                json!({
                    "id": "p",
                    "title": "t",
                    "kind": { "type": "plugin-candidate", "kind": "cargo-target" },
                    "low": {},
                }),
                ev,
            ),
            "isn't a kind plugins report",
        ),
        (
            "a command that isn't a known tool",
            proposal(
                json!({
                    "id": "docker",
                    "title": "docker's images",
                    "kind": { "type": "tool-command", "command": ["docker", "image", "prune", "-f"], "why": "images" },
                    "low": {},
                }),
                ev,
            ),
            "docker isn't a known tool",
        ),
        (
            "no level",
            proposal(
                json!({
                    "id": "nolevel",
                    "title": "t",
                    "kind": { "type": "cache-dir", "path": "~/Library/Caches/Q", "why": "w" },
                }),
                ev,
            ),
            "runs at no level",
        ),
        (
            "targets with no idle",
            proposal(
                json!({ "id": "t", "title": "t", "kind": { "type": "cargo-target" }, "normal": {} }),
                ev,
            ),
            "idleSecs",
        ),
        (
            "a built-in's id for another kind",
            proposal(cache("cargo-target", "~/Library/Caches/T", 0), ev),
            "already a rule of another kind",
        ),
        (
            "no evidence",
            proposal(cache("bare", "~/Library/Caches/Bare", 0), "  "),
            "no evidence",
        ),
        (
            "no title",
            proposal(
                json!({ "id": "x", "title": "", "kind": { "type": "worktree" }, "normal": { "idleSecs": 1 } }),
                ev,
            ),
            "no title",
        ),
    ];
    for (name, p, want) in cases {
        let mut f = merge_built_in(RulesFile {
            proposed: vec![waiting.clone()],
            ..RulesFile::default()
        });
        let id = p.rule.id.clone();
        let got = add_proposals(&mut f, vec![p], H);
        match want {
            "added" => {
                assert_eq!(got.added, std::slice::from_ref(&id), "{name}: {got:?}");
                let p = f.proposed.iter().find(|p| p.rule.id == id).unwrap();
                assert!(p.rule.enabled, "{name}");
            }
            "same" => {
                assert_eq!(got.same, [id], "{name}: {got:?}");
                assert_eq!(f.proposed, std::slice::from_ref(&waiting), "{name}");
            }
            why => {
                assert_eq!(got.refused.len(), 1, "{name}: {got:?}");
                assert!(got.refused[0].1.contains(why), "{name}: {got:?}");
                assert_eq!(f.proposed, std::slice::from_ref(&waiting), "{name}");
            }
        }
    }
}

// What the research file may hold: a list, an object with proposed, or
// one; each read on its own.
#[test]
fn proposals_read() {
    let one = json!({ "rule": cache("a", "~/Library/Caches/A", 0), "evidence": "e" });
    let bad = json!({ "rule": { "id": "b", "title": "t" }, "evidence": "e" });
    let no_ev = json!({ "rule": cache("c", "~/Library/Caches/C", 0) });
    for (name, text, ok, refused) in [
        (
            "a list",
            json!([one, bad, no_ev]).to_string(),
            1,
            vec!["b", "c"],
        ),
        (
            "proposed",
            json!({ "proposed": [one] }).to_string(),
            1,
            vec![],
        ),
        ("one", one.to_string(), 1, vec![]),
        ("not JSON", "{ nope".to_string(), 0, vec![PROPOSALS_FILE]),
        ("a number", "3".to_string(), 0, vec![PROPOSALS_FILE]),
        (
            "unnamed",
            json!([{ "evidence": "e" }]).to_string(),
            0,
            vec!["#1"],
        ),
    ] {
        let (got, no) = parse_proposals(&text);
        assert_eq!(got.len(), ok, "{name}");
        let ids: Vec<&str> = no.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, refused, "{name}: {no:?}");
    }
}

/// rush's folder, for this test's thread only.
struct RushHome {
    _dir: tempfile::TempDir,
}

impl RushHome {
    fn new() -> RushHome {
        let d = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(d.path().canonicalize().unwrap()));
        RushHome { _dir: d }
    }
}

impl Drop for RushHome {
    fn drop(&mut self) {
        rush_state::set_thread_dir(None);
    }
}

// A research file kept as proposals, by and at as given, and only once:
// one dismissed isn't proposed again by the same file.
#[test]
fn research_file_proposed_once() {
    let _home = RushHome::new();
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join(PROPOSALS_FILE);
    std::fs::write(
        &file,
        json!([
            {
                "rule": {
                    "id": "pnpm-store",
                    "title": "pnpm's store, pruned",
                    "kind": { "type": "tool-command", "command": ["pnpm", "store", "prune"], "why": "pnpm's store" },
                    "low": {},
                },
                "evidence": "~/Library/pnpm/store: 14G, pnpm's; prune keeps what projects use",
            },
            { "rule": { "id": "rm", "title": "t", "kind": { "type": "tool-command", "command": ["rm", "-rf", "/"], "why": "w" }, "low": {} }, "evidence": "e" },
        ])
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        propose_file(&d.path().join("none.json"), "x", 1, "s/none.json").unwrap(),
        None
    );
    let at: Ms = 1_800_000_000_000;
    let got = propose_file(
        &file,
        "the cleanup agent's research",
        at,
        "s1/disk-proposals.json",
    )
    .unwrap()
    .unwrap();
    assert_eq!(got.added, ["pnpm-store"]);
    assert_eq!(got.refused.len(), 1, "{got:?}");
    let f = load_rules().unwrap();
    assert_eq!(f.proposed.len(), 1);
    assert_eq!(f.proposed[0].by, "the cleanup agent's research");
    assert_eq!(f.proposed[0].at, at);
    assert!(
        !f.rules.iter().any(|r| r.id == "pnpm-store"),
        "never a rule"
    );
    update_rules(|f| f.proposed.clear()).unwrap();
    let again = propose_file(
        &file,
        "the cleanup agent's research",
        at,
        "s1/disk-proposals.json",
    )
    .unwrap()
    .unwrap();
    assert!(again.already && again.added.is_empty());
    assert!(load_rules().unwrap().proposed.is_empty());
}

const T: Ms = 1_800_000_000_000;

fn found(path: &str, kind: &str, rule: &str) -> Candidate {
    Candidate {
        path: path.into(),
        kind: kind.into(),
        rule: rule.into(),
        facts: Facts {
            mtime: Some(T - 30 * 24 * 3_600_000),
            ..Facts::default()
        },
        ..Candidate::default()
    }
}

// A plugin's candidate in a cache folder a rule removes (yours too) is
// removed by that rule alone; one beside it, or holding it, is as before.
#[test]
fn removed_once() {
    let lv = Some(Params {
        idle_secs: Some(0),
        ..Params::default()
    });
    let rules = vec![
        Rule {
            id: "mine".into(),
            title: "my cache".into(),
            kind: Kind::CacheDir {
                path: "/c".into(),
                why: "a cache".into(),
                each: false,
                go_clean: false,
            },
            enabled: true,
            normal: lv.clone(),
            low: None,
        },
        Rule {
            id: "plugin:js.x".into(),
            title: "x".into(),
            kind: Kind::PluginCandidate {
                kind: "js.x".into(),
            },
            enabled: true,
            normal: lv,
            low: None,
        },
    ];
    let acts = |cands: &[Candidate]| -> Vec<String> {
        plan(cands, &rules, Level::Normal, T)
            .0
            .iter()
            .map(|a| format!("{} {}", a.rule, a.path.display()))
            .collect()
    };
    for (name, cands, want) in [
        (
            "in the folder",
            vec![found("/c", CACHE_DIR, "mine"), found("/c/x", "js.x", "")],
            vec!["mine /c"],
        ),
        (
            "the folder itself",
            vec![found("/c", "js.x", ""), found("/c", CACHE_DIR, "mine")],
            vec!["mine /c"],
        ),
        (
            "beside it",
            vec![found("/c", CACHE_DIR, "mine"), found("/cx", "js.x", "")],
            vec!["mine /c", "plugin:js.x /cx"],
        ),
        (
            "holding it",
            vec![found("/", "js.x", ""), found("/c", CACHE_DIR, "mine")],
            vec!["plugin:js.x /"],
        ),
        (
            "two of a plugin's, nested",
            vec![found("/p", "js.x", ""), found("/p/q", "js.x", "")],
            vec!["plugin:js.x /p"],
        ),
    ] {
        assert_eq!(acts(&cands), want, "{name}");
    }
}
