//! The rush binary: the TUI, `--dump` and `--render`, the host's commands
//! (`host`, `attach`, `view`/`views`, `inbox`, `reload`, `mcp-tools`,
//! `spawn`), `menubar`, `pr`, `watch`, `plugind`, `plugin`, `statusline`,
//! `gate`, `memory`, `room`, `feed`, `events`, `serve`, `remote`, `session`
//! (and `session --on`), `push-later` and a plugin's own commands.
//! After Go's cmd/rush/main.go (usage :46, main :71-200, dump :441, render
//! :478), `open` (session.go:795 openHosted), #reload's restart (:290-300)
//! and the lock the rush that leads holds (:239-248, instances.Lock).
//! The rest of Go's commands (on, off, status, update,
//! why) come with later milestones; see
//! rust/DESIGN.md.

mod flags;
mod hosted;
mod memory;
mod pushlater;
mod room;
mod serve;
mod session;
mod spawn;

use std::time::Instant;

use rush_core::time::now;
use rush_fleet::Predicates;

/// This rush: a commit and its day when built from a checkout, as Go's
/// go-built rush says it (main.go:37 version, update.Info.Short).
fn version() -> String {
    rush_update::current().map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), |i| i.short())
}

const USAGE: &str = "rush — a lighter agents view for Claude Code

  rush             open the view
  rush reload      reload every running rush view in place, as #reload does, and
                   restart every agent on an older rush, each once it's between turns
  rush menubar     put rush in the menu bar: usage, what's working, and
                    questions you can answer from their notification
  rush menubar off take it out again
  rush plugin      sandboxed plugins: list, approve, revoke
  rush gate        queue intensive programs agents run: run, status
  rush memory      the memory notes agents keep: list, add, review, tidy
                    (\"rush memory help\")
  rush feed        the feed agents share, once on (\"rush feed help\")
  rush watch       wait on a PR's checks for an agent, without polling (\"rush watch help\")
  rush remind      send a session a message at a time, waking it (\"rush remind help\")
  rush room        a group chat of fresh agents arguing a topic to a verdict
                    (\"rush room help\")
  rush open <id> --hosted
                    the view of one rush-mode session alone, full width
  rush serve       this machine's sessions to a browser and to rush elsewhere (docs/remote.md)
  rush events      what rush did and why, newest first (\"rush events -h\")
  rush --render WxH [keys…]
                   print one frame of the view
  rush --dump      print what the view sees, for debugging
";

fn main() {
    // The menu bar app is this binary, copied into its bundle: there it
    // runs the app and nothing else.
    if rush_menubar::bundled() {
        if let Err(e) = rush_menubar::app_main() {
            eprintln!("rush: {e}");
            std::process::exit(1);
        }
        return;
    }
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
        // rush's own tools, for a session's agent that runs MCP servers as
        // processes: not meant to be run by hand.
        Some("mcp-tools") => {
            return rush_host::agtools::mcp_tools_cmd(reg.clone(), &args[1..], &mut stderr);
        }
        Some("remind" | "reminders") => {
            return rush_host::cli::remind_cmd(&args[1..], &mut stdout, &mut stderr);
        }
        _ => {}
    }
    match args.first().map(String::as_str) {
        Some("--version" | "version") => println!("rush {}", version()),
        Some("--help" | "-h" | "help") => print!("{USAGE}{}", rush_plugin::cmd::plugins_usage()),
        Some("--dump") => dump(),
        Some("--render") => render(reg, &args[1..]),
        // Its old names still work.
        Some("feed" | "twotter" | "twatter" | "twitter" | "twattr" | "community") => {
            let ctx = rush_room::FeedCtx::new();
            return rush_room::feed_cmd(&ctx, &args[1..], &mut stdin, &mut stdout, &mut stderr);
        }
        // rush pr follow starts this program again as `rush pr follow-run`.
        Some("pr" | "prs") => {
            let mut env = rush_prdrive::cli::Env::from_process();
            env.host = Box::new(PrHost);
            return rush_prdrive::cli::pr_cmd_in(&env, &args[1..], &mut stdout, &mut stderr);
        }
        Some("watch") => return rush_watch::watch_cmd(&args[1..], &mut stdout, &mut stderr),
        Some("events") => {
            let log = rush_events::path();
            return rush_events::events_cmd(&log, &args[1..], now(), &mut stdout, &mut stderr);
        }
        Some("room" | "rooms") => {
            return rush_room::room_cmd(
                &room::ctx(),
                &args[1..],
                &mut stdin,
                &mut stdout,
                &mut stderr,
            );
        }
        // What a session's stand-in for an agent's program runs: not
        // meant to be run by hand.
        Some("spawn") => return spawn::spawn_cmd(reg, &args[1..]),
        // mem is its short name, as Go's (cmd/rush/main.go:134).
        Some("memory" | "mem") => {
            let ask = memory::cheap_ask(reg);
            return memory::memory_cmd(
                reg,
                &memory::Where::from_process(),
                &args[1..],
                &mut stdin,
                terminal(0),
                &ask,
                &mut stdout,
                &mut stderr,
            );
        }
        Some("menubar") => return exit_if(rush_menubar::cli(&args[1..])),
        Some("serve") => return exit_if(serve::serve_cmd(reg, &args[1..])),
        Some("remote") => return exit_if(serve::remote_cmd(&args[1..], &mut stdout)),
        // rush session --on NAME …: on another machine's serve.
        Some("session" | "sessions")
            if args.get(1).map(String::as_str) == Some("--on") && args.len() > 2 =>
        {
            return serve::session_on(&args[2], &args[3..], &mut stdin, &mut stdout, &mut stderr);
        }
        Some("session" | "sessions") => {
            let env = session::Env::process(reg);
            return session::session_cmd(&env, &args[1..], &mut stdin, &mut stdout, &mut stderr);
        }
        Some("plugin" | "plugins") => {
            let mut read_line = || {
                let mut l = String::new();
                let _ = std::io::stdin().read_line(&mut l);
                l
            };
            let mut io = rush_plugin::cmd::Io {
                out: &mut stdout,
                stdin_terminal: terminal(0),
                read_line: &mut read_line,
            };
            return exit_if(rush_plugin::cmd::plugin_cmd(&args[1..], &mut io));
        }
        Some("gate") => {
            let ctx = rush_gate::Ctx::new(std::sync::Arc::new(GateHost::default()));
            return rush_gate::gate_cmd(&ctx, &args[1..]);
        }
        // A git push the gate's hook held until the branch is quiet.
        Some("push-later") => return pushlater::push_later(&args[1..]),
        // The plugin broker. rush starts it when a plugin is approved; it
        // is not meant to be run by hand.
        Some("plugind") => return exit_if(rush_plugind::run()),
        // Claude Code's statusLine command, set up by /statusline in a
        // session: the session's JSON in, one line out.
        Some("statusline") => {
            return exit_if(rush_bundled::statusline::run(
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            ));
        }
        // rush open <id>: the view of that one session alone.
        Some("open") => return exit_if(open_hosted(reg, &args[1..])),
        Some(other) => {
            // A plugin's own commands: its name, where rush's commands
            // take theirs first.
            if let Some(m) = rush_plugin::cli_plugins().get(other) {
                return rush_plugin::cmd::plugin_cli(
                    other,
                    m,
                    &args[1..],
                    &mut stdout,
                    terminal(1),
                    &mut stderr,
                    terminal(2),
                );
            }
            eprintln!("rush: unknown command {other:?}\n\n{USAGE}");
            return 2;
        }
        None => {
            if let Err(e) = lead_lock(
                rush_ui::lead::follow_env(),
                std::env::var("RUSH_VIA").as_deref() == Ok("ssh"),
            ) {
                eprintln!("{e}");
                return 1;
            }
            // Sessions start, and are sent to, through rush's hosts.
            let setup = |m: &mut rush_ui::Model| {
                let h = hosted::RushHosts::new(reg.clone());
                rush_ui::screens::session_core::host::set_hosts(m, std::sync::Arc::new(h));
            };
            match rush_ui::run_with(&version(), setup) {
                Err(e) => {
                    eprintln!("rush: {e}");
                    return 1;
                }
                Ok(Some(env)) => reload(&env),
                Ok(None) => {}
            }
        }
    }
    0
}

/// One rush leads at a time on a folder, as it switches logins and
/// restarts sessions for all of them: the one that leads holds the lock
/// (main.go:239-248, instances.Lock). One that follows (RUSH_FOLLOW, so
/// it runs beside the Go rush, which leads) or runs over SSH (rush
/// serve's) never takes it. Err is what's said when another holds it.
fn lead_lock(follows: bool, ssh: bool) -> Result<(), String> {
    if follows || ssh {
        return Ok(());
    }
    rush_state::instances::lock().map(|_| ()).map_err(|pid| {
        format!(
            "rush is already running (pid {pid}) on {}: one runs at a time, as it switches logins and restarts sessions for all of them. Use that one, or quit it first.",
            rush_state::dir().display()
        )
    })
}

/// #reload: the installed rush in this one's place, the terminal already
/// given back; the sessions' hosts never stopped. It returns only if that
/// couldn't start, having said why (cmd/rush/main.go:290-300).
fn reload(env: &str) {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = rush_sys::selfexe::path() else {
        return;
    };
    let mut f = serde_json::Map::new();
    f.insert("exe".into(), exe.to_string_lossy().into_owned().into());
    rush_events::log("reloading", f.clone());
    rush_events::flush();
    let mut args = std::env::args_os();
    let mut cmd = std::process::Command::new(&exe);
    if let Some(argv0) = args.next() {
        cmd.arg0(argv0);
    }
    cmd.args(args);
    if let Some((k, v)) = env.split_once('=') {
        cmd.env(k, v);
    }
    let err = cmd.exec(); // only returns if it failed
    f.insert("err".into(), err.to_string().into());
    rush_events::log("reload-failed", f);
    eprintln!("rush: #reload couldn't start the installed rush: {err}");
}

/// rush open <id> [--hosted]: the view of that one session alone, as an
/// app embedding rush opens it. Without --hosted it is the same view
/// (cmd/rush/session.go:795-818 openHosted).
fn open_hosted(reg: &rush_core::adapter::Registry, args: &[String]) -> Result<(), String> {
    let id = open_id(args)?;
    let exists = !id.is_empty()
        && !id.contains(['/', '\\'])
        && id != "."
        && id != ".."
        && rush_host::root().join(&id).is_dir();
    if !exists || rush_host::client::read_info(&id).is_err() {
        return Err(format!("session {id} not found"));
    }
    let setup = |m: &mut rush_ui::Model| {
        let h = hosted::RushHosts::new(reg.clone());
        rush_ui::screens::session_core::host::set_hosts(m, std::sync::Arc::new(h));
    };
    rush_ui::run_hosted(&version(), &id, setup).map_err(|e| e.to_string())
}

/// The session id rush open names, its one flag (--hosted, on whatever
/// it's set to) taken as Go's flag package takes it (idAndFlags).
fn open_id(args: &[String]) -> Result<String, String> {
    let mut id = String::new();
    let mut rest = args;
    if let Some(a) = args.first()
        && !a.starts_with('-')
    {
        id = a.clone();
        rest = &args[1..];
    }
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            if id.is_empty() {
                id = it.next().cloned().unwrap_or_default();
            }
            break;
        }
        let Some(flag) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
            if id.is_empty() {
                id = a.clone();
            }
            break; // the flags end at the first argument
        };
        // As Go's strconv.ParseBool takes it.
        const BOOLS: [&str; 12] = [
            "1", "0", "t", "f", "T", "F", "true", "false", "TRUE", "FALSE", "True", "False",
        ];
        let (name, value) = flag
            .split_once('=')
            .map_or((flag, None), |(n, v)| (n, Some(v)));
        if name != "hosted" {
            return Err(format!("flag provided but not defined: -{name}"));
        }
        if let Some(v) = value
            && !BOOLS.contains(&v)
        {
            return Err(format!(
                "invalid boolean value {v:?} for -hosted: parse error"
            ));
        }
    }
    if id.is_empty() {
        return Err("usage: rush session open <id>".into());
    }
    Ok(id)
}

/// Go's exitIf: an error said on stderr, and 1.
fn exit_if(r: Result<(), String>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("rush: {e}");
            1
        }
    }
}

/// Whether fd is a terminal.
fn terminal(fd: i32) -> bool {
    // SAFETY: isatty only asks.
    unsafe { libc::isatty(fd) == 1 }
}

/// What rush pr asks of rush's host: telling a session something goes
/// through it (host.Deliver, now false).
struct PrHost;

impl rush_prdrive::cli::Host for PrHost {
    fn deliver(&self, session: &str, e: &rush_core::event::Exchange) -> Result<(), String> {
        rush_host::deliver(session, e, false).map_err(|e| e.to_string())
    }
}

/// What the gate asks of the rest of rush: the sessions' files, and the
/// search guard and the watch plugin's word on a Bash call while they're
/// on (gate.go:425).
#[derive(Default)]
struct GateHost(rush_gate::FileHost);

impl rush_gate::Host for GateHost {
    fn log_event(&self, id: &str, kind: &str, fields: rush_gate::host::Fields) {
        self.0.log_event(id, kind, fields);
    }
    fn notice(&self, id: &str, text: &str) -> Result<(), String> {
        self.0.notice(id, text)
    }
    fn session_alive(&self, id: &str) -> bool {
        self.0.session_alive(id)
    }
    fn session_name(&self, id: &str) -> Option<String> {
        self.0.session_name(id)
    }
    fn is_agent_program(&self, comm: &str) -> bool {
        self.0.is_agent_program(comm)
    }
    fn search_refusal(&self, command: &str) -> String {
        if !rush_bundled::searchguard::on() {
            return String::new();
        }
        rush_bundled::searchguard::refusal(command).to_string()
    }
    fn watch_redirect(
        &self,
        command: &str,
        background: bool,
        timeout: std::time::Duration,
        exe: &str,
    ) -> (String, String) {
        if !rush_bundled::prwatch::on() {
            return (String::new(), String::new());
        }
        rush_watch::hook::redirect(command, background, timeout, exe)
    }
}

/// Makes the adapters known: everything after reads agents through the
/// registry, never by name.
fn register() {
    let reg = rush_core::adapter::global();
    // The cards under a session (approvals, questions, a stop with a
    // message) reach the session rush runs through the session's own
    // side.
    rush_ui::screens::session_cards::link(rush_ui::screens::session_core::cards_host::host());
    // At home in ~/.claude, whatever CLAUDE_CONFIG_DIR says: every session
    // the list shows runs there (Go's Profiles, ActiveAccount).
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    // Claude Code's memory notes, which #remember, alt+k and the memory
    // view and review write and tidy.
    rush_ui::screens::projects_machine::memreview::register_keeper(
        "claude",
        std::sync::Arc::new(rush_claude::keeper::NoteKeeper),
    );
    // Its saved logins' plan usage, read for the meters (logins.go:80
    // fetchLoginUsage through the adapter's RefreshLogin).
    rush_ui::screens::bars::fetch::set_login_refresher(|path, cfg, lg, offline| {
        rush_claude::Claude::at(rush_claude::Account::home()).refresh_login(path, cfg, lg, offline)
    });
    // Its logins kept in rush's vault: switching, refreshing, adding and
    // forgetting them (state.Logins, the logins agent's LoginKeeper).
    rush_ui::screens::accounts::logins::register_keeper(std::sync::Arc::new(
        rush_claude::Claude::at(rush_claude::Account::home()),
    ));
    rush_codex::register();
    reg.register(rush_pi::Pi::new());
    reg.register(rush_antigravity::Antigravity::new());
    rush_copilot::register(reg);
    rush_ollama::register(reg);
    rush_providers::register(reg);
    rush_acp::register(reg);
    // The plugins that ship inside rush (Go's bundled packages' inits);
    // remote mode's switch installs serve's LaunchAgent.
    rush_bundled::remote::on_switch(rush_remote::plugins::switch_remote);
    rush_bundled::register();
    // The list reads rush's own sessions through the host module.
    // A rush that follows (RUSH_FOLLOW) never starts a lost session's host
    // again: the rush that leads does.
    rush_fleet::use_hosts(|| {
        Box::new(if rush_ui::lead::follow_env() {
            rush_host::hosts::Sessions::following()
        } else {
            rush_host::hosts::Sessions::new()
        })
    });
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
    l.flush();
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

/// `rush --render WxH [keys…]`, with rush's hosts set as the TUI's are,
/// so an opened session's hosted agents are found (render.go:39).
fn render(reg: &rush_core::adapter::Registry, args: &[String]) {
    let (mut w, mut h) = (160u16, 45u16);
    if let Some((a, b)) = args.first().and_then(|s| s.split_once('x')) {
        w = a.parse().unwrap_or(w);
        h = b.parse().unwrap_or(h);
    }
    let keys = rush_ui::parse_keys(args.get(1..).unwrap_or(&[]));
    let setup = |m: &mut rush_ui::Model| {
        let h = hosted::RushHosts::new(reg.clone());
        rush_ui::screens::session_core::host::set_hosts(m, std::sync::Arc::new(h));
    };
    println!("{}", rush_ui::render(&version(), w, h, &keys, setup));
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

    /// rush open's arguments as Go's idAndFlags takes them.
    #[test]
    fn open_takes_an_id_and_hosted() {
        let a = |s: &[&str]| open_id(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        for (args, want) in [
            (&["s1"][..], Ok("s1")),
            (&["s1", "--hosted"], Ok("s1")),
            (&["--hosted", "s1"], Ok("s1")),
            (&["-hosted=false", "s1"], Ok("s1")),
            (&[], Err("usage: rush session open <id>")),
            (&["--hosted"], Err("usage: rush session open <id>")),
            (
                &["s1", "--wide"],
                Err("flag provided but not defined: -wide"),
            ),
            (
                &["s1", "--hosted=maybe"],
                Err("invalid boolean value \"maybe\" for -hosted: parse error"),
            ),
        ] {
            assert_eq!(
                a(args),
                want.map(str::to_string).map_err(str::to_string),
                "{args:?}"
            );
        }
    }

    /// The rush that leads takes the lock, and says so when another holds
    /// it; one that follows, or runs over SSH, never asks for it, so the
    /// Rust rush runs beside the Go one (main.go:239-248).
    #[test]
    fn only_the_rush_that_leads_takes_the_lock() {
        use std::io::{BufRead, Write};
        let dir = rush_state::instances::dir();
        std::fs::create_dir_all(&dir).unwrap();
        // Another rush (the Go one, say) holds it, in a process of its
        // own: held from this process, a child another test was starting
        // could keep the lock after the test let it go. lockf holds the
        // file's flock until its cat's stdin closes.
        let p = dir.join("lock");
        let mut other = std::process::Command::new("/usr/bin/lockf")
            .args(["-k", "-t", "0"])
            .arg(&p)
            .args(["/bin/sh", "-c", "echo held; exec /bin/cat"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut held = String::new();
        std::io::BufReader::new(other.stdout.take().unwrap())
            .read_line(&mut held)
            .unwrap();
        assert_eq!(held, "held\n");
        std::fs::write(&p, "4242").unwrap();
        let err = lead_lock(false, false).unwrap_err();
        assert_eq!(
            err,
            format!(
                "rush is already running (pid 4242) on {}: one runs at a time, as it switches logins and restarts sessions for all of them. Use that one, or quit it first.",
                rush_state::dir().display()
            )
        );
        assert_eq!(
            lead_lock(true, false),
            Ok(()),
            "a rush that follows runs beside it"
        );
        assert_eq!(lead_lock(false, true), Ok(()), "over SSH it follows too");
        // Let go, the rush that leads takes it and writes its pid.
        let mut stdin = other.stdin.take().unwrap();
        stdin.flush().unwrap();
        drop(stdin);
        assert!(other.wait().unwrap().success());
        assert_eq!(lead_lock(false, false), Ok(()));
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            std::process::id().to_string()
        );
    }

    /// A session that isn't there isn't opened: no view starts.
    #[test]
    fn open_needs_the_session() {
        let reg = rush_core::adapter::Registry::new();
        let s = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            open_hosted(&reg, &s(&["nope"])),
            Err("session nope not found".into())
        );
        assert_eq!(
            open_hosted(&reg, &s(&["../x"])),
            Err("session ../x not found".into())
        );
    }
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
