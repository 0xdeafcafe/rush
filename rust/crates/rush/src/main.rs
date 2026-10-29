//! The rush binary: the TUI, `--dump` and `--render`, the host's commands
//! (`host`, `attach`, `view`/`views`, `inbox`, `reload`), `rush watch` and
//! `rush pr`. After Go's cmd/rush/main.go (usage :46, main :71-200, dump
//! :441, render :478). The rest of Go's commands (on, off, session…) come with
//! later milestones; see rust/DESIGN.md.

mod room;

use std::time::Instant;

use rush_core::time::now;
use rush_fleet::Predicates;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "rush — a lighter agents view for Claude Code

  rush             open the view
  rush reload      reload every running rush view in place, as #reload does, and
                   restart every agent on an older rush, each once it's between turns
  rush watch       wait on a PR's checks for an agent, without polling (\"rush watch help\")
  rush --render WxH [keys…]
                   print one frame of the view
  rush --dump      print what the view sees, for debugging
";

fn main() {
    raise_open_files();
    register();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The host renames itself: see rush_host::run.
    rush_events::set_process(args.first().map_or("ui", String::as_str));
    let code = run(&args);
    rush_events::flush();
    if code != 0 {
        std::process::exit(code);
    }
}

/// Runs the command args name; its exit status.
fn run(args: &[String]) -> i32 {
    let reg = rush_core::adapter::global();
    // Not locked: a host's threads write to them while it runs.
    let (mut stdin, mut stdout, mut stderr) =
        (std::io::stdin(), std::io::stdout(), std::io::stderr());
    match args.first().map(String::as_str) {
        Some("attach") => return rush_host::cli::attach_cmd(reg, &args[1..], &mut stderr),
        // rush host run <id>: the detached process a rush-mode session
        // lives in. rush starts it; it is not meant to be run by hand.
        // rush host preflight <id> asks this rush, newly installed,
        // whether it can run the session before the old host stops.
        Some("host") => {
            return rush_host::cli::host_cmd(reg, &args[1..], &mut stdout, &mut stderr);
        }
        Some("view" | "views") => {
            return rush_host::cli::view_cmd(&args[1..], &mut stdin, &mut stdout, &mut stderr);
        }
        // A session's hook after each tool call: not meant to be run by
        // hand.
        Some("inbox") => {
            return rush_host::cli::inbox_cmd(&args[1..], &mut stdin, &mut stdout, &mut stderr);
        }
        Some("reload") => return rush_host::cli::reload_cmd(&mut stdout),
        _ => {}
    }
    match args.first().map(String::as_str) {
        Some("--version" | "version") => println!("rush {VERSION}"),
        Some("--help" | "-h" | "help") => print!("{USAGE}"),
        Some("--dump") => dump(),
        Some("--render") => render(&args[1..]),
        // rush watch and rush pr, each with Go's exit codes. rush pr
        // follow starts this program again as `rush pr follow-run`.
        Some("watch") => {
            let code =
                rush_watch::watch_cmd(&args[1..], &mut std::io::stdout(), &mut std::io::stderr());
            std::process::exit(code);
        }
        Some("pr" | "prs") => {
            let code =
                rush_prdrive::pr_cmd(&args[1..], &mut std::io::stdout(), &mut std::io::stderr());
            std::process::exit(code);
        }
        Some(other) => {
            eprintln!("rush: unknown command {other:?}\n\n{USAGE}");
            std::process::exit(2);
        }
        None => {
            if let Err(e) = rush_ui::run(VERSION) {
                eprintln!("rush: {e}");
                return 1;
            }
        }
    }
    0
}

/// Makes the adapters known: everything after reads agents through the
/// registry, never by name.
fn register() {
    let reg = rush_core::adapter::global();
    // At home in ~/.claude, whatever CLAUDE_CONFIG_DIR says: every session
    // the list shows runs there (Go's Profiles, ActiveAccount).
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    rush_codex::register();
    reg.register(rush_pi::Pi::new());
    reg.register(rush_antigravity::Antigravity::new());
    rush_copilot::register(reg);
    rush_ollama::register(reg);
    rush_providers::register(reg);
}

/// Raises the soft limit on open files to the most allowed. launchd starts
/// programs at 256, and rush watches a folder per project and a file per
/// open transcript: past the limit a watch can't be opened, and that part
/// of the list falls back to being read every few seconds. Go's runtime
/// does the same at startup (os/rlimit.go).
fn raise_open_files() {
    // SAFETY: zeroed is a valid rlimit; getrlimit fills it.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
        return;
    }
    let mut want = r.rlim_max;
    // macOS refuses more than kern.maxfilesperproc, even under an
    // unlimited hard limit.
    #[cfg(target_os = "macos")]
    {
        let mut n: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: an int out-buffer of the size given.
        let ok = unsafe {
            libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&mut n as *mut libc::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        if ok && n > 0 {
            want = want.min(n as libc::rlim_t);
        }
    }
    if want <= r.rlim_cur {
        return;
    }
    r.rlim_cur = want;
    // SAFETY: a valid rlimit.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &r) };
}

/// Prints what the view sees as Go's dump does (main.go:441), so the two
/// diff: each agent, each account, the machine's processes and the
/// machine's totals.
fn dump() {
    let mut l = rush_fleet::Loader::from_state(rush_core::adapter::global().clone());
    let snap = l.load_unsampled(now());
    let t0 = Instant::now();
    l.scan(&snap.agents);
    let scan = t0.elapsed();
    l.load(now());
    // A second on, the processes' CPU has a baseline (main.go:458).
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut snap = l.load(now());
    let now = now();
    for a in &snap.agents {
        println!("{}", dump_line(a, now));
    }
    if let Some(av) = snap.accounts.first_mut()
        && !av.daemon
    {
        // Claude Code's daemon, asked through its socket until its adapter
        // says so itself (Joiner::service_up).
        av.daemon = rush_claude::Account::from_env().daemon_running();
    }
    for av in &snap.accounts {
        print!("{}", account_line(av));
        println!();
    }
    for r in &snap.machine.rows {
        println!("{}", proc_line(r));
    }
    println!(
        "machine mem={:.0}M cpu={:.1}% spares={} scan={}",
        snap.machine.total_mem as f64 / (1u64 << 20) as f64,
        snap.machine.total_cpu,
        snap.machine.spares,
        go_duration(scan.as_nanos() as i64)
    );
}

/// `account %s daemon=%v live=%d agents=%d spend=$%.2f today=$%.2f` and
/// each window's ` %s=%.0f%%`.
fn account_line(av: &rush_fleet::AccountView) -> String {
    let mut s = format!(
        "account {} daemon={} live={} agents={} spend=${:.2} today=${:.2}",
        av.folder.name, av.daemon, av.live, av.agents, av.spend, av.today
    );
    for w in &av.quota.windows {
        s += &format!(" {}={:.0}%", w.label, w.percent);
    }
    s
}

/// `proc %6d role=%d %-40.40s %5.1f%% %7.0fM n=%d`
fn proc_line(r: &rush_fleet::ProcRow) -> String {
    format!(
        "proc {:>6} role={} {} {:>5.1}% {:>7.0}M n={}",
        r.pid,
        r.role.number(),
        pad40(&r.label),
        r.cpu,
        r.mem as f64 / (1u64 << 20) as f64,
        r.procs
    )
}

/// `%-40.40s`: at most 40 characters, padded to 40.
fn pad40(s: &str) -> String {
    let name: String = s.chars().take(40).collect();
    let pad = 40 - name.chars().count();
    name + &" ".repeat(pad)
}

/// `%-8s %-8s %-40.40s %6.1f%% %7.0fM %8.2f %9s %6s %d prs`
fn dump_line(a: &rush_core::agent::Agent, now: i64) -> String {
    let round = |ms: i64, unit: i64| (ms + unit / 2).div_euclid(unit) * unit;
    format!(
        "{:<8} {:<8} {} {:>6.1}% {:>7.0}M {:>8.2} {:>9} {:>6} {} prs",
        a.job.id,
        a.job.state.as_str(),
        pad40(&a.display_name),
        a.cpu,
        a.mem as f64 / (1u64 << 20) as f64,
        a.spend.cost,
        go_duration(round(a.elapsed(now), 60_000) * 1_000_000),
        go_duration(round(a.age(now), 1000) * 1_000_000),
        a.prs.len(),
    )
}

/// A duration in nanoseconds as Go's time.Duration.String writes it.
fn go_duration(ns: i64) -> String {
    if ns == 0 {
        return "0s".into();
    }
    let neg = ns < 0;
    let u = ns.unsigned_abs();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if u < 1_000_000_000 {
        let (unit, div) = if u < 1_000 {
            ("ns", 1)
        } else if u < 1_000_000 {
            ("µs", 1_000)
        } else {
            ("ms", 1_000_000)
        };
        out.push_str(&frac(u, div));
        out.push_str(unit);
        return out;
    }
    let h = u / 3_600_000_000_000;
    let m = u / 60_000_000_000 % 60;
    let s = u % 60_000_000_000;
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if h > 0 || m > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&frac(s, 1_000_000_000));
    out.push('s');
    out
}

/// n/div with its fraction, trailing zeros dropped.
fn frac(n: u64, div: u64) -> String {
    let whole = n / div;
    let rest = n % div;
    if rest == 0 {
        return whole.to_string();
    }
    let digits = div.ilog10() as usize;
    let f = format!("{rest:0digits$}");
    format!("{whole}.{}", f.trim_end_matches('0'))
}

/// `rush --render WxH [keys…]`
fn render(args: &[String]) {
    let (mut w, mut h) = (160u16, 45u16);
    if let Some((a, b)) = args.first().and_then(|s| s.split_once('x')) {
        w = a.parse().unwrap_or(w);
        h = b.parse().unwrap_or(h);
    }
    let keys = rush_ui::parse_keys(args.get(1..).unwrap_or(&[]));
    println!("{}", rush_ui::render(VERSION, w, h, &keys));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_files_are_raised() {
        raise_open_files();
        // SAFETY: as in raise_open_files.
        let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out-pointer.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) }, 0);
        assert!(
            r.rlim_cur > 256 || r.rlim_cur == r.rlim_max,
            "{}",
            r.rlim_cur
        );
    }

    #[test]
    fn go_durations() {
        let cases: &[(i64, &str)] = &[
            (0, "0s"),
            (6_000_000_000, "6s"),
            (18 * 60_000_000_000, "18m0s"),
            (2 * 3_600_000_000_000 + 28 * 60_000_000_000, "2h28m0s"),
            (
                1084 * 3_600_000_000_000 + 44 * 60_000_000_000 + 12_000_000_000,
                "1084h44m12s",
            ),
            (200_078_167, "200.078167ms"),
            (1_500, "1.5µs"),
            (1_500_000_000, "1.5s"),
        ];
        for &(ns, want) in cases {
            assert_eq!(go_duration(ns), want, "{ns}");
        }
    }

    #[test]
    fn dump_lines_line_up() {
        let mut a = rush_core::agent::Agent::default();
        a.job.id = "39f8a973".into();
        a.job.state = rush_core::agent::State::Idle;
        a.display_name = "39f8a973-d4".into();
        a.spend.cost = 0.91;
        let l = dump_line(&a, 0);
        assert_eq!(
            l,
            "39f8a973 idle     39f8a973-d4                                 0.0%       0M     0.91        0s     0s 0 prs"
        );
    }

    #[test]
    fn proc_and_account_lines() {
        let r = rush_fleet::ProcRow {
            pid: 812,
            role: rush_fleet::Role::Daemon,
            label: "daemon".into(),
            cpu: 1.25,
            mem: 48 << 20,
            procs: 3,
            ..Default::default()
        };
        assert_eq!(
            proc_line(&r),
            "proc    812 role=1 daemon                                     1.2%      48M n=3"
        );
        let mut av = rush_fleet::AccountView::default();
        av.folder.name = "default".into();
        av.live = 2;
        av.agents = 9;
        av.spend = 12.345;
        av.quota.windows.push(rush_core::usage::Window {
            label: "5h".into(),
            percent: 41.6,
            ..Default::default()
        });
        assert_eq!(
            account_line(&av),
            "account default daemon=false live=2 agents=9 spend=$12.35 today=$0.00 5h=42%"
        );
    }
}
