//! After internal/bundled/ts/ts_test.go: what needs a real language
//! server runs only where one is installed. tests/ts.rs drives a stand-in
//! server end to end.

use super::*;

fn real_root(files: &[(&str, &str)]) -> Option<(tempfile::TempDir, String)> {
    let t = tempfile::tempdir().unwrap();
    let root = t
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for (name, body) in files {
        let p = std::path::Path::new(&root).join(name);
        std::fs::write(p, body).unwrap();
    }
    resolve(&root, "auto").ok()?;
    Some((t, root))
}

// ts_test.go TestPoolCapsAServer: a server past its cap is stopped, the
// call told why, and the next call gets a fresh one. Skipped where no
// server is installed.
#[test]
fn pool_caps_a_server() {
    let Some((_t, root)) = real_root(&[("tsconfig.json", "{}"), ("a.ts", "export const a = 1;\n")])
    else {
        return;
    };
    let f = format!("{root}/a.ts");
    let ctx = Ctx::background();
    let p = Pool::new(
        &ctx,
        Options {
            engine: "auto".into(),
            cap: 1 << 20,
            ..Default::default()
        },
    );
    let s = p.server_for(&ctx, &f).unwrap();
    p.look();
    assert!(
        s.wait_done(Duration::from_secs(20)),
        "not stopped past its cap"
    );
    assert!(
        s.why().is_some_and(|w| w.contains("past its")),
        "why = {:?}",
        s.why()
    );
    p.set_options(Options {
        engine: "auto".into(),
        ..Default::default()
    });
    let again = p.server_for(&ctx, &f).unwrap();
    assert!(!Arc::ptr_eq(&again, &s), "no fresh server");
    let a = Ask {
        file: f.clone(),
        line: 1,
        symbol: "a".into(),
        ..Default::default()
    };
    let out = run_op(&ctx, &p, op_named("hover").unwrap(), &a, &root).unwrap();
    assert!(out.contains('a'), "hover = {out}");
    p.close_all();
}

#[test]
fn cli_and_tools_lists() {
    let specs = cli_specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "definition",
            "references",
            "implementation",
            "hover",
            "callers",
            "callees",
            "outline",
            "diagnostics",
            "find-symbol",
            "rename",
            "rename-file",
            "status"
        ]
    );
    assert_eq!(
        specs[0].usage,
        "--file F --line N (--symbol S | --character C)"
    );
    assert_eq!(
        specs[0].description,
        "Where the symbol at a position is declared"
    );
    assert_eq!(
        specs[9].usage,
        "--file F --line N (--symbol S | --character C) --new-name N"
    );
    assert_eq!(specs[10].usage, "--file F --to PATH");
    assert_eq!(
        specs[8].description,
        "Declarations whose name matches query, across the checkout"
    );
    let tools = tool_list();
    assert_eq!(
        tools[9]["inputSchema"]["required"],
        serde_json::json!(["file", "line", "new_name"])
    );
    assert_eq!(
        tools[8]["inputSchema"]["properties"]["query"]["type"],
        "string"
    );
}

#[test]
fn cli_refuses_what_it_doesnt_know() {
    let ctx = Ctx::background();
    let p = Pool::new(&ctx, Options::default());
    let r = |cmd: &str, args: &[&str]| {
        run_cli(
            &ctx,
            &p,
            &CliRun {
                command: cmd.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
                ..Default::default()
            },
        )
    };
    assert_eq!(r("status", &[]).stdout, "no language servers running\n");
    let out = r("nope", &[]);
    assert_eq!(
        (out.exit, out.stderr.as_str()),
        (2, "rush ts: no command nope\n")
    );
    let out = r("hover", &["--wat", "x"]);
    assert_eq!(
        (out.exit, out.stderr.as_str()),
        (2, "rush ts: unknown --wat\n")
    );
    let out = r("hover", &["--file=/nowhere/at/all.ts", "--line", "1"]);
    assert_eq!(out.exit, 1);
    assert!(out.stderr.contains("is in no checkout"), "{out:?}");
}
