//! Ported from internal/keymap/keymap_test.go:1-280, plus keybindings.json
//! reading and writing (file.go).

use super::*;
use Context::{Any, Global, List, Pages, Prompt, Session};

fn act(id: &str, c: Context, keys: &[&str]) -> Action {
    Action {
        keys: keys.iter().map(|k| k.to_string()).collect(),
        ..Action::new(id, c)
    }
}

fn acts() -> Vec<Action> {
    vec![
        act("quit", Global, &["ctrl+q"]),
        act("list.stop", List, &["ctrl+x"]),
        act("list.up", List, &["up"]),
        act("session.send", Session, &["ctrl+s"]),
        act("session.stop", Session, &["ctrl+x"]),
        act("session.enter", Session, &["enter"]),
        act("command:drafts", Any, &[]),
        Action {
            source: "haven".into(),
            ..act("plugin:haven.open", Any, &[])
        },
    ]
}

fn file(b: &[(&str, &[&str])]) -> File {
    File {
        bindings: b
            .iter()
            .map(|(id, ks)| (id.to_string(), ks.iter().map(|k| k.to_string()).collect()))
            .collect(),
    }
}

fn none() -> BTreeMap<String, Vec<String>> {
    BTreeMap::new()
}

fn seq(ks: &[&str]) -> Seq {
    ks.iter().copied().collect()
}

fn strs(ks: &[&str]) -> Vec<String> {
    ks.iter().map(|k| k.to_string()).collect()
}

#[test]
fn parse_keys() {
    for (input, want) in [
        ("ctrl+x d", "ctrl+x d"),
        ("Cmd+K", "super+K"),
        ("shift+ctrl+up", "ctrl+shift+up"),
        ("meta+alt+Enter", "alt+super+enter"),
        ("ctrl++", "ctrl++"),
        ("option+s", "alt+s"),
    ] {
        let s = parse(input);
        assert_eq!(
            s.as_ref().map(Seq::to_string),
            Ok(want.to_string()),
            "parse({input:?})"
        );
    }
    for bad in ["", "hyper+x", "ctrl+", "a b c d"] {
        assert!(parse(bad).is_err(), "parse({bad:?}) should fail");
    }
}

#[test]
fn parse_errors_say_why() {
    for (input, want) in [
        ("", "an empty key"),
        ("a b c d", r#""a b c d": a chord of at most 3 keys"#),
        (
            "hyper+x",
            r#""hyper+x": key "hyper+x": "hyper" is not a modifier (use ctrl, alt, shift or cmd)"#,
        ),
        (
            "ctrl+",
            r#""ctrl+": key "ctrl+" has no key after its modifiers"#,
        ),
    ] {
        assert_eq!(parse(input).unwrap_err().to_string(), want);
    }
}

#[test]
fn types_a_character() {
    for (k, want) in [
        ("a", true),
        ("space", true),
        ("+", true),
        ("shift+a", true),
        ("é", true),
        ("ctrl+a", false),
        ("enter", false),
        ("shift+up", false),
    ] {
        assert_eq!(types(k), want, "types({k:?})");
    }
}

#[test]
fn defaults_pass_through() {
    let m = Map::build(&acts(), &File::default(), &none());
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    assert_eq!(m.resolve(&[Session], &[], "ctrl+s").key, "ctrl+s");
    assert_eq!(m.resolve(&[List], &[], "x").key, "x", "typing");
}

#[test]
fn rebind_stands_in_for_default() {
    let m = Map::build(
        &acts(),
        &file(&[("session.send", &["ctrl+enter"])]),
        &none(),
    );
    assert_eq!(m.resolve(&[Session], &[], "ctrl+enter").key, "ctrl+s");
    // ctrl+s no longer sends, and does nothing else.
    let r = m.resolve(&[Session], &[], "ctrl+s");
    assert!(r.key.is_empty() && r.run.is_empty(), "freed = {r:?}");
    // But only in the Session: the list's ctrl+s isn't this one.
    assert_eq!(m.key_text("session.send"), "ctrl+enter");
    assert!(m.changed("session.send"));
}

#[test]
fn swap() {
    let m = Map::build(
        &acts(),
        &file(&[("session.send", &["enter"]), ("session.enter", &["ctrl+s"])]),
        &none(),
    );
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    assert_eq!(m.resolve(&[Session], &[], "enter").key, "ctrl+s");
    assert_eq!(m.resolve(&[Session], &[], "ctrl+s").key, "enter");
}

#[test]
fn chord() {
    let m = Map::build(
        &acts(),
        &file(&[
            ("command:drafts", &["ctrl+g d"]),
            ("session.stop", &["ctrl+g x"]),
        ]),
        &none(),
    );
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    let r = m.resolve(&[Session], &[], "ctrl+g");
    assert_eq!(r.pending, seq(&["ctrl+g"]), "prefix = {r:?}");
    let p = r.pending.keys();
    assert_eq!(m.resolve(&[Session], p, "d").run, "command:drafts");
    assert_eq!(
        m.resolve(&[Session], p, "x").key,
        "ctrl+x",
        "chord to default"
    );
    assert_eq!(m.resolve(&[Session], p, "q").missed.to_string(), "ctrl+g q");
    // Commands work from the list too.
    assert_eq!(m.resolve(&[List], &[], "ctrl+g").pending.len(), 1);
}

#[test]
fn chord_takes_default_prefix() {
    // ctrl+x is list.stop's; a chord of yours starting with it takes it.
    let m = Map::build(
        &acts(),
        &file(&[("command:drafts", &["ctrl+x d"])]),
        &none(),
    );
    assert_eq!(m.resolve(&[List], &[], "ctrl+x").pending.len(), 1);
    assert_eq!(m.key_text("list.stop"), "");
}

#[test]
fn refused() {
    let m = Map::build(
        &acts(),
        &file(&[
            ("command:drafts", &["d"]), // types
            // Yours are taken in name order: plugin:haven.open takes quit's
            // default, so session.send finds it yours already, and is
            // refused.
            ("plugin:haven.open", &["ctrl+q"]),
            ("session.send", &["ctrl+q"]),
            ("nope", &["ctrl+e"]),
        ]),
        &none(),
    );
    let mut got: Vec<&str> = m.problems().iter().map(|p| p.action.as_str()).collect();
    got.sort();
    assert_eq!(
        got,
        ["command:drafts", "nope", "session.send"],
        "{:?}",
        m.problems()
    );
    let words: Vec<String> = m.problems().iter().map(Problem::to_string).collect();
    assert_eq!(
        words,
        [
            "command:drafts on d: it types a character; start a chord with a key that doesn't, ctrl+x say",
            "nope: no such action",
            "session.send on ctrl+q: already plugin:haven.open",
        ]
    );
}

#[test]
fn plugin_suggestion_only_where_free() {
    let suggested: BTreeMap<String, Vec<String>> = [
        ("plugin:haven.open".to_string(), strs(&["ctrl+s", "alt+h"])),
        ("session.send".to_string(), strs(&["alt+z"])), // not the plugin's to suggest
    ]
    .into();
    let m = Map::build(&acts(), &File::default(), &suggested);
    assert_eq!(m.key_text("plugin:haven.open"), "alt+h");
    assert_eq!(
        m.key_text("session.send"),
        "ctrl+s",
        "a plugin moved rush's key"
    );
}

#[test]
fn conflicts() {
    let m = Map::build(&acts(), &File::default(), &none());
    assert_eq!(
        m.conflicts("command:drafts", &seq(&["ctrl+x"])),
        ["list.stop", "session.stop"]
    );
    assert_eq!(m.conflicts("session.send", &seq(&["ctrl+q"])), ["quit"]);
    assert!(m.conflicts("nope", &seq(&["ctrl+q"])).is_empty());
}

#[test]
fn defaults_are_clean() {
    let d = defaults();
    let m = Map::build(&d, &File::default(), &none());
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    let mut seen = std::collections::HashSet::new();
    for a in &d {
        assert!(seen.insert(a.id.clone()), "{} twice", a.id);
        for k in &a.keys {
            let s = parse(k).unwrap();
            assert_eq!(
                &s.to_string(),
                k,
                "{}: {k:?} isn't written as rush reads it",
                a.id
            );
        }
    }
}

#[test]
fn own_default_keys_arrive_as_pressed() {
    let m = Map::build(
        &[act("place.next", Global, &[".", ">", "ctrl+\\"])],
        &File::default(),
        &none(),
    );
    for k in [".", ">", "ctrl+\\"] {
        let r = m.resolve(&[], &[], k);
        assert_eq!(r.key, k, "{k:?} became {r:?}");
    }
}

// The Prompt's keys come first while something's typed, and are run; the
// list's own have the same keys back once it's empty, or once you move the
// Prompt's.
#[test]
fn prompt_before_list() {
    let acts = [
        act("list.groupby", List, &["ctrl+s"]),
        Action {
            runs: true,
            ..act("prompt.stash", Prompt, &["ctrl+s"])
        },
    ];
    let m = Map::build(&acts, &File::default(), &none());
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    assert_eq!(
        m.resolve(&[Prompt, List], &[], "ctrl+s").run,
        "prompt.stash"
    );
    let r = m.resolve(&[List], &[], "ctrl+s");
    assert!(r.key == "ctrl+s" && r.run.is_empty(), "empty = {r:?}");
    let m = Map::build(&acts, &file(&[("prompt.stash", &["ctrl+x s"])]), &none());
    let r = m.resolve(&[Prompt, List], &[], "ctrl+s");
    assert!(r.key == "ctrl+s" && r.run.is_empty(), "moved = {r:?}");
    assert_eq!(
        m.resolve(&[Prompt, List], &strs(&["ctrl+x"]), "s").run,
        "prompt.stash"
    );
    assert!(m.starts(&[Prompt], "ctrl+x"));
    assert!(!m.starts(&[List], "ctrl+x"));
}

fn first(k: &str) -> &str {
    k.split_whitespace().next().unwrap_or("")
}

// rush's own keys don't clash, and none needs the option key: every action
// has one without ⌥, and no two share a key (or a chord's first key) where
// both are live. The Prompt's shadow the list's by design.
#[test]
fn defaults_build_clean() {
    let d = defaults();
    let m = Map::build(&d, &File::default(), &none());
    assert!(m.problems().is_empty(), "{:?}", m.problems());
    // The terminal's (ctrl+c, and enter, tab, backspace, esc as ctrl keys)
    // and the boxes' editing keys, taken nowhere a box can have the keys.
    let taken = [
        "ctrl+c", "ctrl+m", "ctrl+i", "ctrl+h", "ctrl+[", "ctrl+j", "ctrl+a", "ctrl+e", "ctrl+w",
        "ctrl+u",
    ];
    for a in &d {
        if !a.keys.is_empty() && !a.keys.iter().any(|k| !first(k).contains("alt+")) {
            panic!("{} has only ⌥ keys: {:?}", a.id, a.keys);
        }
        for k in &a.keys {
            assert!(
                !(taken.contains(&first(k)) && a.context != Pages),
                "{}: {k} is the terminal's or a box's",
                a.id
            );
        }
    }
    for c in CONTEXTS {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for a in &d {
            if !scope(a.context).contains(&c) && (a.context != Global || c == Any) {
                continue;
            }
            for k in &a.keys {
                if let Some(other) = seen.insert(k, &a.id) {
                    panic!("{c}: {k} is both {other} and {}", a.id);
                }
            }
        }
        for (k, id) in &seen {
            if let Some((first, _)) = k.split_once(' ')
                && let Some(other) = seen.get(first)
            {
                panic!("{c}: {first} is {other}, and {id}'s chord {k} starts with it");
            }
        }
    }
    assert_eq!(
        m.key_text("session.stash"),
        m.key_text("prompt.stash"),
        "the stash differs between a Session and the Prompt"
    );
}

// A default chord arrives as its action's key rush's handling knows: ctrl+]
// then h as ⌥h.
#[test]
fn default_chord_arrives_as_its_key() {
    let m = Map::build(&defaults(), &File::default(), &none());
    let r = m.resolve(&[Session], &[], "ctrl+]");
    assert_eq!(r.pending, seq(&["ctrl+]"]), "ctrl+] = {r:?}");
    assert_eq!(m.resolve(&[Session], &strs(&["ctrl+]"]), "h").key, "alt+h");
    assert_eq!(m.resolve(&[List], &[], "ctrl+d").key, "ctrl+d");
}

#[test]
fn context_names_and_titles() {
    for c in CONTEXTS {
        assert_eq!(Context::from_name(c.as_str()), Some(c));
    }
    assert_eq!(
        Context::Pages.title(),
        "Pages: Efficiency, Settings, Projects, Wall"
    );
    assert_eq!(command_id("drafts"), "command:drafts");
    assert_eq!(plugin_id("haven", "open"), "plugin:haven.open");
}

#[test]
fn file_with() {
    let f = file(&[("a", &["ctrl+a"])]);
    let g = f.with("b", Some(vec![]));
    assert_eq!(g.bindings.get("b"), Some(&vec![]), "an empty list unbinds");
    assert_eq!(
        g.with("a", None).bindings.len(),
        1,
        "none goes back to the default"
    );
    assert_eq!(f.bindings.len(), 1, "f is left as it was");
}

#[test]
fn file_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sub").join("keybindings.json");
    assert_eq!(
        load_from(&p).unwrap(),
        File::default(),
        "none is no bindings"
    );
    let f = file(&[("session.send", &["ctrl+x s"]), ("list.stop", &[])]);
    save_to(&p, &f).unwrap();
    // As Go's state.WriteJSON writes it: indented two spaces, keys in order.
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "{\n  \"bindings\": {\n    \"list.stop\": [],\n    \"session.send\": [\n      \"ctrl+x s\"\n    ]\n  }\n}"
    );
    assert_eq!(load_from(&p).unwrap(), f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let leftovers = std::fs::read_dir(p.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 1, "no temp file left behind");
}

#[test]
fn file_reads_what_go_writes() {
    for (input, want) in [
        (r#"{"bindings":null}"#, File::default()),
        (r#"{}"#, File::default()),
        (
            r#"{"Bindings":{"quit":null,"zen":["ctrl+z"]},"other":1}"#,
            file(&[("quit", &[]), ("zen", &["ctrl+z"])]),
        ),
    ] {
        assert_eq!(
            serde_json::from_str::<File>(input).unwrap(),
            want,
            "{input}"
        );
    }
}
