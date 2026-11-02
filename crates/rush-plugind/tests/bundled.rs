//! A bundled plugin runs without an approval or a sandbox, as rush itself,
//! and stops when it's turned off. This test's own binary stands in for
//! rush: the broker starts a bundled plugin as `<exe> plugin run <name>`,
//! which main here answers by running it.
//! Ported from internal/plugind/bundled_test.go; the plugins inside the
//! broker are Rust-only.

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
        start: None,
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
        println!("bundled_plugins_run_inside: test");
        return;
    }
    bundled_plugin_runs();
    println!("test bundled_plugin_runs ... ok");
    bundled_plugins_run_inside();
    println!("test bundled_plugins_run_inside ... ok");
}

/// This process's footprint, in bytes.
fn footprint() -> u64 {
    let me = std::process::id() as i32;
    let mut t = rush_sys::proc::snapshot();
    t.fill(None, &[me]);
    t.procs.get(&me).map_or(0, |p| p.footprint)
}

/// How many of this process's children are alive.
fn children() -> usize {
    let me = std::process::id() as i32;
    rush_sys::proc::snapshot()
        .procs
        .values()
        .filter(|p| p.ppid == me)
        .count()
}

/// A bundled plugin with a start runs inside the broker: no process, and
/// no thread of its own while it's quiet; it answers as one in a process
/// does; one that panics is caught and started again, the broker
/// unharmed.
fn bundled_plugins_run_inside() {
    const N: usize = 14;
    let home = tempfile::Builder::new()
        .prefix("rpi-")
        .tempdir_in("/tmp")
        .unwrap();
    rush_state::set_thread_dir(Some(home.path().canonicalize().unwrap()));
    let h: Handler = Arc::new(|_, _, method, params| match method {
        "cli.run" => {
            let r: CliRun = from_raw(params).map_err(rush_plugin::Error::server)?;
            if r.command == "boom" {
                panic!("asked to");
            }
            to_raw(&CliResult {
                stdout: format!("{}\n", r.args.join(" ")),
                ..Default::default()
            })
            .map(Some)
        }
        _ => to_raw(&serde_json::json!({})).map(Some),
    });
    let afters = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let regs: Vec<Registered> = (0..N)
        .map(|i| {
            let (h, afters) = (h.clone(), afters.clone());
            let start: rush_plugin::StartFn = Arc::new(move |s| {
                let conn = Conn::new(s, Some(h.clone())).map_err(|e| e.to_string())?;
                let afters = afters.clone();
                Ok(rush_plugin::Started {
                    conn,
                    after: Box::new(move || {
                        afters.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                })
            });
            rush_plugin::register_bundle(Bundle {
                manifest: Manifest {
                    name: format!("bundle-in-{i}"),
                    command: vec!["rush".into()],
                    cli: vec![
                        CliSpec {
                            name: "echo".into(),
                            usage: "[words]".into(),
                            description: "says them back".into(),
                        },
                        CliSpec {
                            name: "boom".into(),
                            usage: "".into(),
                            description: "panics".into(),
                        },
                    ],
                    ..Default::default()
                },
                optional: false,
                run: None,
                start: Some(start),
                switched: None,
            })
        })
        .collect();
    let logger = Arc::new(rush_events::Logger::new(Some(
        home.path().join("events.jsonl"),
    )));
    let threads_before = rush_sys::threads::count();
    let footprint_before = footprint();
    let b = Broker::new(
        Arc::new(FileHosts::logging_to(logger.clone())),
        Log::discard(),
        Scope::with_events(logger),
    );
    b.reload();
    for i in 0..N {
        let st = b
            .wait_status(&format!("bundle-in-{i}"), Duration::from_secs(10), |s| {
                s.state == "running"
            })
            .unwrap_or_else(|| panic!("not running: {:?}", b.status()));
        assert_eq!(
            st.pid,
            std::process::id() as i64,
            "inside, its pid is the broker's"
        );
    }
    assert_eq!(children(), 0, "inside the broker, it has no process");
    // Quiet, they hold no thread each: at most a handful (the shared
    // reader, the reaper), whatever N is. The threads that started them
    // end just after they're running, so it waits for those to go.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let added = loop {
        let added = rush_sys::threads::count().saturating_sub(threads_before);
        if added <= 4 || std::time::Instant::now() > deadline {
            break added;
        }
        std::thread::yield_now();
    };
    println!(
        "{N} bundled plugins inside the broker: {added} threads, {} KB footprint added",
        footprint().saturating_sub(footprint_before) >> 10
    );
    assert!(added <= 4, "{N} quiet plugins inside added {added} threads");

    // It answers as one in a process does.
    let ctx = Ctx::background().with_timeout(Duration::from_secs(10));
    let run = |command: &str, args: &[&str]| CliRun {
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let res = b.cli(&ctx, "bundle-in-0", run("echo", &["hi"])).unwrap();
    assert_eq!(res.stdout, "hi\n");
    // A panic in it answers the call with an error: the broker and the
    // others go on.
    let e = b.cli(&ctx, "bundle-in-1", run("boom", &[])).unwrap_err();
    assert!(e.message.contains("panicked: asked to"), "{e:?}");
    let res = b.cli(&ctx, "bundle-in-2", run("echo", &["still"])).unwrap();
    assert_eq!(res.stdout, "still\n");

    // Turned off, it stops: its connection closes and what it does after
    // runs.
    rush_plugin::set_bundled("bundle-in-0", false).unwrap();
    b.reload();
    assert!(b.status().iter().all(|s| s.name != "bundle-in-0"));
    assert_eq!(afters.load(std::sync::atomic::Ordering::SeqCst), 1);

    // One that panics as it starts is caught, written to its log as its
    // process's stderr would have it, and started again.
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start: rush_plugin::StartFn = Arc::new(move |s| {
        if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            panic!("at start");
        }
        let conn = Conn::new(s, None).map_err(|e| e.to_string())?;
        Ok(rush_plugin::Started::serving(conn))
    });
    let panics = rush_plugin::register_bundle(Bundle {
        manifest: Manifest {
            name: "bundle-in-panics".into(),
            command: vec!["rush".into()],
            ..Default::default()
        },
        optional: false,
        run: None,
        start: Some(start),
        switched: None,
    });
    b.reload();
    let st = b
        .wait_status("bundle-in-panics", Duration::from_secs(10), |s| {
            s.state == "running"
        })
        .unwrap_or_else(|| panic!("not started again: {:?}", b.status()));
    assert_eq!(st.restarts, 1, "{st:?}");
    let log =
        std::fs::read_to_string(rush_plugin::log_path("bundle-in-panics")).unwrap_or_default();
    assert!(log.contains("panicked: at start"), "log: {log:?}");
    assert_eq!(children(), 0);
    b.stop();
    panics.forget();
    for r in regs {
        r.forget();
    }
    rush_state::set_thread_dir(None);
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
