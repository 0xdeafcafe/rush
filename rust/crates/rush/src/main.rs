//! The rush binary: the TUI, `--dump` and `--render`. After Go's
//! cmd/rush/main.go (usage :46, main :71, dump :441, render :478). The
//! rest of Go's commands (on, off, host, session, menubar…) come with
//! later milestones; see rust/DESIGN.md.

use std::time::Instant;

use rush_core::time::now;
use rush_fleet::Predicates;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "rush — a lighter agents view for Claude Code

  rush             open the view
  rush --render WxH [keys…]
                   print one frame of the view
  rush --dump      print what the view sees, for debugging
";

fn main() {
    raise_open_files();
    register();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "version") => println!("rush {VERSION}"),
        Some("--help" | "-h" | "help") => print!("{USAGE}"),
        Some("--dump") => dump(),
        Some("--render") => render(&args[1..]),
        Some(other) => {
            eprintln!("rush: unknown command {other:?}\n\n{USAGE}");
            std::process::exit(2);
        }
        None => {
            if let Err(e) = rush_ui::run(VERSION) {
                eprintln!("rush: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Makes the adapters known: everything after reads agents through the
/// registry, never by name.
fn register() {
    rush_core::adapter::global().register(rush_claude::Claude::new());
    rush_codex::register();
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

/// Prints each agent as Go's dump does, so the two diff.
fn dump() {
    let t0 = Instant::now();
    let mut d = rush_ui::worker::Discovery::new();
    d.load(now(), true);
    let scan = t0.elapsed();
    // A second on, the processes' CPU has a baseline (main.go:458).
    std::thread::sleep(std::time::Duration::from_secs(1));
    let f = d.load(now(), false);
    let now = now();
    let (mut live, mut agents, mut spend, mut today) = (0, 0, 0.0, 0.0);
    for a in &f.agents {
        println!("{}", dump_line(a, now));
        if !a.past {
            agents += 1;
        }
        if a.job.live() {
            live += 1;
        }
        spend += a.spend.cost;
        today += a.spend.today;
    }
    let daemon = rush_claude::Account::from_env().daemon_running();
    println!(
        "account default daemon={daemon} live={live} agents={agents} spend=${spend:.2} today=${today:.2}"
    );
    println!(
        "machine mem=0M cpu=0.0% spares=0 scan={}",
        go_duration(scan.as_nanos() as i64)
    );
}

/// `%-8s %-8s %-40.40s %6.1f%% %7.0fM %8.2f %9s %6s %d prs`
fn dump_line(a: &rush_core::agent::Agent, now: i64) -> String {
    let name: String = a.display_name.chars().take(40).collect();
    let pad = 40 - name.chars().count();
    let round = |ms: i64, unit: i64| (ms + unit / 2).div_euclid(unit) * unit;
    format!(
        "{:<8} {:<8} {}{} {:>6.1}% {:>7.0}M {:>8.2} {:>9} {:>6} {} prs",
        a.job.id,
        a.job.state.as_str(),
        name,
        " ".repeat(pad),
        a.cpu,
        a.mem as f64 / (1u64 << 20) as f64,
        a.spend.cost,
        go_duration(round(a.elapsed(now), 60_000) * 1_000_000),
        go_duration(round(a.age(now), 1000) * 1_000_000),
        0,
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
}
