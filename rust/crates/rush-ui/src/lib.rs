//! rush's screens: the agent list and a session opened read-only, on
//! rush-draw's idle-free loop (DESIGN.md "Threads"). After Go's
//! internal/ui. `run` opens the TUI; `render` draws one frame, as Go's
//! `rush --render` does (internal/ui/render.go:12 Frame).

mod chores;
mod clanker;
pub mod config;
pub mod frame;
mod header;
pub mod hosted;
pub mod lead;
pub mod list;
mod model;
pub mod screens;
mod session;
pub mod style;
pub mod worker;

use std::io;

use rush_draw::{Headless, Key, KeyMods, Loop, Term};

pub use model::Model;
pub use worker::{Fleet, Note};

/// Opens the TUI and runs it until quit.
pub fn run(version: &str) -> io::Result<()> {
    run_with(version, |_| {}).map(|_| ())
}

/// Opens the TUI with what the binary gives it first (setup: the hosts
/// sessions run in, screens::session_core::host::set_hosts), and runs it
/// until quit. While it runs it's listed for `rush reload`, whose
/// SIGUSR1 is #reload (cmd/rush/main.go:266-277). What it returns is the
/// variable #reload asked the next rush to start with (Model::reload_env),
/// for the binary to run the installed rush in its place.
pub fn run_with(version: &str, setup: impl FnOnce(&mut Model)) -> io::Result<Option<String>> {
    run_model(version, None, setup)
}

/// The view of one rush-mode session alone (`rush open <id>`, Go's
/// NewHosted and openHosted, cmd/rush/session.go:795-818): no list
/// unless ctrl+6 shows it, and only ctrl+q quits (hosted.rs). It isn't
/// listed for `rush reload`, as Go's isn't.
pub fn run_hosted(version: &str, id: &str, setup: impl FnOnce(&mut Model)) -> io::Result<()> {
    run_model(version, Some(id), setup).map(|_| ())
}

fn run_model(
    version: &str,
    hosted: Option<&str>,
    setup: impl FnOnce(&mut Model),
) -> io::Result<Option<String>> {
    let lp: Loop<Note> = Loop::new();
    let mut m = Model::with_workers(version, lp.sender());
    m.cfg = config::Config::load();
    if let Some(id) = hosted {
        hosted::set(&mut m, id);
    }
    setup(&mut m);
    // Links rush knows the kind of say where they stand (model.go:448).
    screens::cmdbar::linkkinds::start(&m);
    // Anything holding the UI thread over 40 ms is noted in stalls.log.
    frame::uistall::watch_ui(config::dir().join("stalls.log"));
    if hosted.is_some() {
        // Hosted leaves past conversations out of each reading until its
        // list is shown (NewHosted's loader.SkipPast(true)).
        let tx = lp.sender().lane(1);
        let give: Box<dyn FnOnce(worker::DiscoveryCtl) + Send> = Box::new(move |ctl| {
            let _ = tx.send(worker::Note::Apply(Box::new(move |m: &mut Model| {
                hosted::set_discovery(m, ctl)
            })));
        });
        worker::spawn_discovery_with(lp.sender(), Some(give));
    } else {
        worker::spawn_discovery(lp.sender());
    }
    // As Go's Init (internal/ui/model.go:548): plugins connected
    // (startHooks, pluginui.go:36), and the layout kept, or asked for the
    // first time (startView, model.go:574).
    // A rush that follows only talks to a broker already running: the
    // rush that leads starts it (lead.rs).
    let hooks = if m.following() {
        rush_plugind::hooks::Client::with_dial(Box::new(rush_plugind::hooks::dial_running))
    } else {
        rush_plugind::hooks::Client::new()
    };
    lead::log_lead(&m);
    screens::plugin_ui::start(&mut m, hooks);
    // Hosted is only the one session: nothing about the app as a whole
    // (model.go:556 Init).
    if hosted.is_none() {
        screens::settings::sheets::start_view(&mut m);
    }
    let mut term = Term::new()?;
    let (w, h) = term.size()?;
    m.w = w as usize;
    m.h = h as usize;
    lp.read_input()?;
    // SIGUSR1 is `rush reload`: #reload, asked for from outside. Only a
    // view that's listed is signalled, and it's listed once this is set.
    let listed = match hosted {
        None => {
            reload_on_signal(&lp);
            Some(rush_state::instances::register())
        }
        Some(_) => None,
    };
    let r = lp.run(&mut term, &mut m);
    // Given back before a reload's exec, which starts with SIGUSR1
    // unhandled (main.go:283).
    drop(term);
    drop(listed);
    // Quitting saves the spend read so far, so the next start needn't
    // read it all again (internal/ui/keys.go:644 quit: sc.Flush()); any
    // command paused goes on, so none is left stopped with nobody to
    // start it (resumeAll).
    screens::projects_machine::shells::resume_all();
    worker::flush_spend();
    r.map(|()| m.reload_env())
}

/// Each SIGUSR1 is #reload (Go's ReloadMsg, reload.go:83): a thread
/// blocks on a pipe the signal writes to, so it costs nothing until then.
fn reload_on_signal(lp: &Loop<Note>) {
    use std::io::Read;
    let Ok((mut r, w)) = std::os::unix::net::UnixStream::pair() else {
        return;
    };
    if signal_hook::low_level::pipe::register(libc::SIGUSR1, w).is_err() {
        return;
    }
    let tx = lp.sender().lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-reload".into())
        .spawn(move || {
            let mut b = [0u8; 16];
            while matches!(r.read(&mut b), Ok(n) if n > 0) {
                if !tx.send(Note::Apply(Box::new(|m: &mut Model| m.reload(false)))) {
                    return;
                }
            }
        });
}

/// A key as `rush --render` takes it: tab, down, enter, ?, text=…,
/// ctrl+x, alt+x, and the rest of the arrow and page keys.
pub fn parse_keys(args: &[String]) -> Vec<(Key, KeyMods)> {
    let mut out = Vec::new();
    for k in args {
        let key = match k.as_str() {
            "tab" => Key::Tab,
            "down" => Key::Down,
            "up" => Key::Up,
            "left" => Key::Left,
            "right" => Key::Right,
            "enter" => Key::Enter,
            "esc" => Key::Esc,
            "space" => Key::Char(' '),
            "pgdown" => Key::PageDown,
            "pgup" => Key::PageUp,
            "home" => Key::Home,
            "end" => Key::End,
            "?" => Key::Char('?'),
            "shift+left" => {
                out.push((Key::Left, KeyMods::SHIFT));
                continue;
            }
            "shift+right" => {
                out.push((Key::Right, KeyMods::SHIFT));
                continue;
            }
            _ => {
                if let Some(t) = k.strip_prefix("text=") {
                    out.extend(t.chars().map(|c| (Key::Char(c), KeyMods::NONE)));
                } else if let Some(c) = k.strip_prefix("ctrl+").and_then(|r| r.chars().next()) {
                    out.push((Key::Char(c), KeyMods::CTRL));
                } else if let Some(c) = k.strip_prefix("alt+").and_then(|r| r.chars().next()) {
                    out.push((Key::Char(c), KeyMods::ALT));
                }
                continue;
            }
        };
        out.push((key, KeyMods::NONE));
    }
    out
}

/// One frame w×h of `m` after `keys`, as ANSI text, or plain when asked.
pub fn frame(m: &mut Model, w: u16, h: u16, keys: &[(Key, KeyMods)], ansi: bool) -> String {
    m.w = w as usize;
    m.h = h as usize;
    for &(k, mods) in keys {
        m.key(k, mods);
    }
    let mut hd = Headless::new(w, h);
    hd.draw(|c| rush_draw::App::<Note>::draw(m, c));
    if ansi { hd.ansi() } else { hd.plain() }
}

/// `rush --render WxH [keys…]`: the list as it is now, one frame. With
/// RUSH_RENDER_SELECT, the agent whose name or id has it, opened.
pub fn render(version: &str, w: u16, h: u16, keys: &[(Key, KeyMods)]) -> String {
    let mut m = Model::new(version);
    m.cfg = config::Config::load();
    // Rust-only: RUSH_RENDER_HOSTED=<id> draws `rush open <id>`'s view,
    // to set beside Go's NewHosted frame.
    if let Ok(id) = std::env::var("RUSH_RENDER_HOSTED")
        && !id.is_empty()
    {
        hosted::set(&mut m, &id);
    }
    screens::accounts::state::load_read_only(&mut m);
    // What the ticks would have read by the first frame: the list and the
    // machine's agent processes, read twice half a second apart so CPU is
    // what each used between (render.go:15-17 Frame: refreshNow, a 500ms
    // sleep, refreshNow).
    let mut d = worker::Discovery::new();
    m.set_fleet(d.load(rush_core::time::now(), true));
    screens::projects_machine::read_machine(&mut m);
    std::thread::sleep(std::time::Duration::from_millis(500));
    m.set_fleet(d.load(rush_core::time::now(), false));
    // The rooms, read where the list is (snapload.go:74 refreshNow).
    m.load_rooms();
    screens::projects_machine::read_machine_again(&mut m);
    // What git says of the list's projects, as Go's Frame has it.
    screens::list_extras::landing::ask_folders(&mut m);
    if let Ok(want) = std::env::var("RUSH_RENDER_SELECT")
        && !want.is_empty()
    {
        let want = want.to_lowercase();
        if let Some(i) = m.agents.iter().position(|a| {
            a.display_name.to_lowercase().contains(&want) || a.job.id.starts_with(&want)
        }) {
            m.sel = m.agents[i].key.clone();
            m.w = w as usize;
            m.h = h as usize;
            m.open(i);
            // RUSH_RENDER_VIEW opens one of its views: overview, changes…
            // (Go's render.go:42).
            // Its runs read as the ticks would (Go's refreshSubs), so a
            // subagents view is there to open.
            if let Some(key) = screens::session_core::open_key(&m) {
                screens::session_core::subs::refresh(&mut m, &key);
            }
            if let Ok(v) = std::env::var("RUSH_RENDER_VIEW")
                && let Some(key) = screens::session_core::open_key(&m)
            {
                screens::session_core::views::show_view(&mut m, &key, &v);
            }
            // Picked, not opened: beside the list when there's room for
            // both, the list keeping the keys (render.go:24 preview).
            if screens::list_extras::split::side_by_side(&m) {
                screens::list_extras::split::leave_pane(&mut m);
            }
        }
    }
    frame(&mut m, w, h, keys, true)
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
