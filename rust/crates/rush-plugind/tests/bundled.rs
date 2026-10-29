//! A bundled plugin runs without an approval or a sandbox, as rush itself,
//! and stops when it's turned off. This test's own binary stands in for
//! rush: the broker starts a bundled plugin as `<exe> plugin run <name>`,
//! which main here answers by running it.
//! Ported from internal/plugind/bundled_test.go.

use std::sync::Arc;
use std::time::Duration;

use rush_plugin::rpc::{Handler, from_raw, to_raw};
use rush_plugin::{
    Bundle, CleanupReport, CliResult, CliRun, CliSpec, Conn, Ctx, Manifest, Registered,
    SERVICE_CLEANUP_REPORT, UI_NOTIFY,
};
use rush_plugind::{Broker, FileHosts, Log, Scope};

fn register_e2e() -> Registered {
    let h: Handler = Arc::new(|_, _, method, params| match method {
        "cli.run" => {
            let r: CliRun = from_raw(params).map_err(rush_plugin::Error::server)?;
            to_raw(&CliResult {
                stdout: format!("{}\n", r.args.join(" ")),
                stderr: r.command.clone(),
                exit: r.args.len() as i64,
            })
            .map(Some)
        }
        "service.call" => to_raw(&CleanupReport {
            title: "E2E".into(),
            report: "all clear".into(),
            ..Default::default()
        })
        .map(Some),
        _ => to_raw(&serde_json::json!({})).map(Some),
    });
    rush_plugin::register_bundle(Bundle {
        manifest: Manifest {
            name: "bundle-e2e".into(),
            command: vec!["rush".into()],
            ui: vec![UI_NOTIFY.into()],
            cli: vec![CliSpec {
                name: "echo".into(),
                usage: "[words]".into(),
                description: "says them back".into(),
            }],
            provides: vec![SERVICE_CLEANUP_REPORT.into()],
            ..Default::default()
        },
        optional: false,
        run: Some(Arc::new(move |s| {
            let conn = Conn::new(s, Some(h.clone())).map_err(|e| e.to_string())?;
            conn.wait();
            Ok(())
        })),
        switched: None,
    })
}

fn main() {
    rush_testenv::assert_sandboxed(); // every test in a throwaway home
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "plugin" && args[2] == "run" {
        let _r = register_e2e();
        if let Err(e) = rush_plugin::run_bundled(&args[3]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if args.iter().skip(1).any(|a| a == "--list") {
        println!("bundled_plugin_runs: test");
        return;
    }
    bundled_plugin_runs();
    println!("test bundled_plugin_runs ... ok");
}

fn bundled_plugin_runs() {
    let home = tempfile::Builder::new()
        .prefix("rpd-")
        .tempdir_in("/tmp")
        .unwrap();
    rush_state::set_thread_dir(Some(home.path().canonicalize().unwrap()));
    let reg = register_e2e();
    assert!(
        rush_plugin::enabled().contains_key("bundle-e2e"),
        "bundled plugin not enabled"
    );
    let logger = Arc::new(rush_events::Logger::new(Some(
        home.path().join("events.jsonl"),
    )));
    let b = Broker::new(
        Arc::new(FileHosts::logging_to(logger.clone())),
        Log::discard(),
        Scope::with_events(logger),
    );
    b.reload();
    let st = b
        .wait_status("bundle-e2e", Duration::from_secs(10), |s| {
            s.state == "running"
        })
        .unwrap_or_else(|| panic!("not running: {:?}", b.status()));
    assert!(st.pid > 0);

    // Its CLI command runs in it, and what it answers comes back.
    let ctx = Ctx::background().with_timeout(Duration::from_secs(10));
    let run = |command: &str, args: &[&str]| CliRun {
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let res = b
        .cli(&ctx, "bundle-e2e", run("echo", &["hello", "there"]))
        .unwrap();
    assert!(
        res.stdout == "hello there\n" && res.stderr == "echo" && res.exit == 2,
        "cli: {res:?}"
    );
    assert!(
        b.cli(&ctx, "bundle-e2e", run("rm", &[])).is_err(),
        "a command it doesn't declare should be refused"
    );
    // It answers the service it provides; nobody answers one it doesn't.
    let answers = b.ask(&ctx, SERVICE_CLEANUP_REPORT, "");
    assert_eq!(answers.len(), 1, "ask: {answers:?}");
    assert_eq!(answers[0].plugin, "bundle-e2e");
    let rep: CleanupReport = from_raw(answers[0].result.as_deref()).unwrap();
    assert_eq!(rep.report, "all clear");
    assert!(b.ask(&ctx, "other.thing", "").is_empty());
    // The one asking isn't asked itself.
    assert!(
        b.ask(&ctx, SERVICE_CLEANUP_REPORT, "bundle-e2e").is_empty(),
        "asked itself"
    );

    rush_plugin::set_bundled("bundle-e2e", false).unwrap();
    b.reload();
    assert!(
        b.status().iter().all(|s| s.name != "bundle-e2e"),
        "turned off, it should stop"
    );
    assert!(b.done().err().is_some(), "with nothing to run, it stops");
    reg.forget();
    rush_state::set_thread_dir(None);
}
