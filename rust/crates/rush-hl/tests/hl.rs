// Port of photon/hl/hl_test.go and the pure parts of
// internal/convo/highlight_test.go (TestChanged, TestLangFor, TestCodePrefix,
// TestHighlightMarkdown's classes).

use rush_hl::code::{changed, code_prefix, line_no};
use rush_hl::{Class, Lang, State};

type Run = (String, Class);

/// Lexes src line by line in lang, carrying state, and returns each line's
/// runs. Fails if a line's runs do not tile it exactly.
fn lines(lang: &str, src: &[&str]) -> Vec<Vec<Run>> {
    let l = Lang::for_name(lang).unwrap_or_else(|| panic!("for_name({lang:?}) = None"));
    let mut st = State::default();
    src.iter().map(|s| tiles(l, &mut st, s)).collect()
}

fn tiles(l: &Lang, st: &mut State, s: &str) -> Vec<Run> {
    let mut rs = Vec::new();
    let mut pos = 0;
    let mut last = None;
    l.line(st, s, |i, j, c| {
        assert!(
            i == pos && j > i && j <= s.len(),
            "{} {s:?}: run [{i},{j}) after {pos}",
            l.name
        );
        assert_ne!(last, Some(c), "{s:?}: runs of one class not merged");
        last = Some(c);
        rs.push((s[i..j].to_string(), c));
        pos = j;
    });
    assert_eq!(pos, s.len(), "{} {s:?}: runs end at {pos}", l.name);
    rs
}

fn has(rs: &[Run], text: &str, c: Class) -> bool {
    rs.iter().any(|(t, k)| t == text && *k == c)
}

#[test]
fn classes() {
    use Class::*;
    let tests: &[(&str, &str, &str, &str, Class)] = &[
        ("go keyword", "go", "func main() {", "func", Keyword),
        ("go call", "go", "func main() {", "main", Function),
        (
            "go call site",
            "go",
            "\tfmt.Println(\"hi\")",
            "Println",
            Function,
        ),
        ("go string", "go", "x := \"hello\"", "\"hello\"", String),
        ("go number", "go", "x := 42", "42", Number),
        ("go constant", "go", "return nil", "nil", Number),
        ("go type", "go", "var x int", "int", Type),
        ("go comment", "go", "x := 1 // note", "// note", Comment),
        ("go inline block", "go", "a /* b */ c", "/* b */", Comment),
        ("py comment", "py", "x = 1  # note", "# note", Comment),
        ("py keyword", "py", "def f(x):", "def", Keyword),
        ("py hash in word", "py", "a#b", "a#b", Plain),
        (
            "json key",
            "json",
            "  \"name\": \"loafer\"",
            "\"name\"",
            Function,
        ),
        (
            "json value",
            "json",
            "  \"name\": \"loafer\"",
            "\"loafer\"",
            String,
        ),
        ("json constant", "json", "  \"ok\": true", "true", Number),
        (
            "sh variable",
            "sh",
            "echo \"$HOME\" $USER",
            "$USER",
            Function,
        ),
        ("sh comment", "sh", "ls # list", "# list", Comment),
        ("sh keyword", "sh", "if true; then", "if", Keyword),
        ("yaml key", "yaml", "name: loafer", "name", Function),
        ("md heading", "md", "## Title", "## Title", Keyword),
        ("md code", "md", "use `go test` now", "`go test`", String),
        // A word in two lists takes the later one's class.
        ("js void is a type", "ts", "let x: void", "void", Type),
        ("rust self is a type", "rs", "self.x", "self", Type),
        (
            "css dashed word",
            "css",
            "@font-face {",
            "font-face",
            Keyword,
        ),
        ("php hash comment", "php", "$x = 1; #c", "#c", Comment),
        ("php variable", "php", "$x = 1;", "$x", Function),
        ("sql comment", "sql", "SELECT 1 -- c", "-- c", Comment),
        ("caps type", "go", "var x Foo", "Foo", Type),
        ("toml key", "toml", "name = 1", "name", Function),
        ("sh quote after word", "sh", "echo it's", "'s", String),
        ("go quote after word", "go", "it's", "it's", Plain),
        ("sh braced var", "sh", "${HOME}/x", "${HOME}", Function),
        ("float", "go", "x := 1.5", "1.5", Number),
    ];
    for &(name, lang, src, text, want) in tests {
        let rs = &lines(lang, &[src])[0];
        assert!(
            has(rs, text, want),
            "{name}: want {text:?} as {want:?} in {rs:?}"
        );
    }
}

#[test]
fn block_comment_spans_lines() {
    let got = lines("go", &["x := 1 /* start", "still comment", "end */ y := 2"]);
    assert!(has(&got[0], "/* start", Class::Comment), "{:?}", got[0]);
    assert_eq!(got[1], vec![("still comment".into(), Class::Comment)]);
    assert!(has(&got[2], "end */", Class::Comment) && has(&got[2], "2", Class::Number));
}

#[test]
fn multi_line_string() {
    let got = lines("go", &["s := `abc", "def`; x := 1"]);
    assert!(has(&got[1], "def`", Class::String) && has(&got[1], "1", Class::Number));
    let got = lines("py", &["s = \"\"\"abc", "def", "\"\"\"", "x = 1"]);
    assert!(has(&got[1], "def", Class::String) && has(&got[3], "1", Class::Number));
}

#[test]
fn markdown_fence() {
    let got = lines("md", &["text", "```go", "func f() {}", "```", "func"]);
    assert!(has(&got[2], "func", Class::Keyword), "{:?}", got[2]);
    assert!(has(&got[1], "```go", Class::Comment));
    assert!(!has(&got[4], "func", Class::Keyword));
}

#[test]
fn markdown_fence_state() {
    let md = Lang::for_name("md").unwrap();
    let mut st = State::default();
    tiles(md, &mut st, "```go");
    assert_eq!(st.fence(), Lang::for_name("go"));
    // A fence's inside isn't Markdown, and its own state carries.
    assert!(!has(
        &tiles(md, &mut st, "# not a heading"),
        "# not a heading",
        Class::Keyword
    ));
    tiles(md, &mut st, "x := `raw");
    assert!(has(&tiles(md, &mut st, "still`"), "still`", Class::String));
    assert!(has(
        &tiles(md, &mut st, "return nil"),
        "return",
        Class::Keyword
    ));
    // ~~~ inside a ``` fence doesn't close it.
    tiles(md, &mut st, "~~~");
    assert!(st.fence().is_some());
    tiles(md, &mut st, "```");
    assert_eq!(st, State::default(), "the fence should close");
    // A fence with no language is plain code.
    tiles(md, &mut st, "```");
    assert_eq!(st.fence(), None);
    assert_eq!(
        tiles(md, &mut st, "# plain"),
        vec![("# plain".into(), Class::String)]
    );
}

#[test]
fn markdown_classes() {
    use Class::*;
    let md = Lang::for_name("md").unwrap();
    let cases: &[(&str, &[(&str, Class)])] = &[
        ("## 5. Work completed", &[("## 5. Work completed", Keyword)]),
        (
            "- **process-stores:** two `new` ones",
            &[
                ("- ", Number),
                ("**process-stores:**", Type),
                ("`new`", String),
            ],
        ),
        ("| a.ts | rewritten |", &[("|", Comment), (" a.ts ", Plain)]),
        ("|---|---|", &[("|---|---|", Comment)]),
        (
            "see [the spec](https://x.y) now",
            &[
                ("[", Comment),
                ("the spec", Function),
                ("](https://x.y)", Comment),
            ],
        ),
        ("12. twelfth", &[("12. ", Number)]),
        ("> quoted", &[("> quoted", Comment)]),
        ("---", &[("---", Comment)]),
        ("#nope", &[("#nope", Plain)]),
    ];
    for &(line, want) in cases {
        let mut st = State::default();
        let rs = tiles(md, &mut st, line);
        for &(t, c) in want {
            assert!(has(&rs, t, c), "{line:?}: missing {t:?} as {c:?} in {rs:?}");
        }
    }
}

#[test]
fn for_name() {
    let same: &[(&str, &[&str])] = &[
        (
            "go",
            &[
                "golang",
                "go",
                "main.go",
                "GO",
                " go ",
                "go.mod",
                "/a/b/c.go",
            ],
        ),
        ("py", &["py", "python", "python3", "x/y.py"]),
        ("sh", &["bash", "sh", "Makefile", "script.zsh"]),
        ("js", &["ts", "tsx", "javascript", "a.mjs"]),
        ("json", &["jsonc", "x.json"]),
        // convo's TestLangFor
        ("js", &["App.tsx", "TypeScript"]),
        ("go", &["internal/ui/view.go"]),
        ("yaml", &["config.yaml"]),
        ("md", &["README.md"]),
    ];
    for &(want, names) in same {
        let w = Lang::for_name(want).unwrap_or_else(|| panic!("for_name({want:?}) = None"));
        for n in names {
            assert_eq!(Lang::for_name(n), Some(w), "for_name({n:?})");
        }
    }
    for n in [
        "unknownlang",
        "",
        "README",
        "x.unknown",
        "noext",
        "dir.go/x",
    ] {
        assert_eq!(Lang::for_name(n), None, "for_name({n:?})");
    }
    // Long and non-ASCII names lower-case too.
    let long = format!("{}/X.PY", "a".repeat(100));
    assert_eq!(Lang::for_name(&long), Lang::for_name("py"));
    assert_eq!(Lang::for_name("é/X.GO"), Lang::for_name("go"));
}

/// The runs cover each line exactly, in order, on char boundaries, in every
/// language, for awkward lines as well as ordinary ones.
#[test]
fn tiles_every_language() {
    let samples = [
        "",
        " ",
        "\t",
        "func main() {",
        "x := \"unterminated",
        "x := 'a'",
        "s := `raw",
        "\"esc \\\" ape\" \\",
        "/* open",
        "close */ x",
        "// c",
        "# c",
        "-- c",
        "$",
        "${",
        "${x}",
        "1.5e10 0x1F 3.",
        "héllo wörld → 世界",
        "a-b-c",
        "key: value",
        "- item: 1",
        "`tick",
        "```",
        "## h",
        "> q",
        "| a | b |",
        "|---|---|",
        "[t](u) **b** `c`",
        "[",
        "[x]",
        "**",
        "1. one",
        "\"\"\"",
        "'''x",
        "'it's",
        "\\",
        "\"\\",
        "\"\\é\" x",
        "'\\→",
        "```é",
        "~~~ rust",
        "$é",
    ];
    for (name, l) in rush_hl::names() {
        let mut st = State::default();
        for s in samples {
            tiles(l, &mut st, s);
        }
        // Again with every language fenced in Markdown.
        let md = Lang::for_name("md").unwrap();
        let mut st = State::default();
        tiles(md, &mut st, &format!("```{name}"));
        for s in samples.iter().filter(|s| !s.starts_with("```")) {
            tiles(md, &mut st, s);
        }
    }
    let mut st = State::default();
    let mut runs = Vec::new();
    rush_hl::line(None, &mut st, "abc", |i, j, c| runs.push((i, j, c)));
    assert_eq!(runs, vec![(0, 3, Class::Plain)]);
}

#[test]
fn changed_spans() {
    type Case = (&'static str, &'static str, Option<(usize, usize)>);
    let cases: &[Case] = &[
        ("return x + 1", "return x + 2", Some((11, 12))),
        ("foo(bar)", "foo(baz, qux)", Some((6, 7))),
        ("abc", "xyz", None),
        ("same", "same", None),
        ("héllo wörld", "héllo world", Some((8, 10))), // é is two bytes
        ("", "x", None),
        ("x", "", None),
    ];
    for &(a, b, want) in cases {
        assert_eq!(changed(a, b), want, "changed({a:?}, {b:?})");
    }
}

#[test]
fn code_prefixes() {
    let cases: &[(&str, &str, &str)] = &[
        ("    12→\tfunc x()", "    12→", ""),
        ("12\tfunc x()", "12\t", ""),
        (
            "internal/ui/view.go:1749:func x",
            "internal/ui/view.go:1749:",
            "internal/ui/view.go",
        ),
        ("a.go-12-  ctx", "a.go-12-", "a.go"),
        ("no prefix here", "", ""),
        ("12:30 meeting", "12:", ""),
        ("a-b/c.go:12:x", "a-b/c.go:12:", "a-b/c.go"),
        ("824-\treturn", "824-", ""),
    ];
    for &(l, want, path) in cases {
        let (n, p) = code_prefix(l);
        assert_eq!((&l[..n], p), (want, path), "code_prefix({l:?})");
    }
    for (p, want) in [
        ("    12→", 12),
        ("a.go:1749:", 1749),
        ("", 0),
        ("x", 0),
        ("824-", 824),
    ] {
        assert_eq!(line_no(p), want, "line_no({p:?})");
    }
}

/// A rough speed check, as Go's BenchmarkLine: `cargo test -p rush-hl
/// --release -- --ignored --nocapture bench`.
#[test]
#[ignore]
fn bench_line() {
    let s = "\tif err := srv.Handle(ctx, \"message\", 42); err != nil { // report it upstream";
    let l = Lang::for_name("go").unwrap();
    let n = 2_000_000;
    let mut total = 0;
    let t = std::time::Instant::now();
    for _ in 0..n {
        let mut st = State::default();
        l.line(&mut st, std::hint::black_box(s), |i, j, _| total += j - i);
    }
    let el = t.elapsed();
    assert!(total > 0);
    println!("line: {:.1} ns/op", el.as_nanos() as f64 / n as f64);

    // A whole Go file, as convo's BenchmarkHighlight.
    let src = include_str!("../../../../internal/convo/render.go");
    let reps = 20;
    let t = std::time::Instant::now();
    for _ in 0..reps {
        let mut st = State::default();
        for line in src.lines() {
            l.line(&mut st, line, |i, j, _| total += j - i);
        }
    }
    let el = t.elapsed();
    let mbs = (src.len() * reps) as f64 / el.as_secs_f64() / 1e6;
    println!(
        "render.go: {:.2} ms/op, {mbs:.0} MB/s",
        el.as_secs_f64() * 1e3 / reps as f64
    );
}
