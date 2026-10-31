//! rush's screens: the agent list and a session opened read-only, on
//! rush-draw's idle-free loop (DESIGN.md "Threads"). After Go's
//! internal/ui. `run` opens the TUI; `render` draws one frame, as Go's
//! `rush --render` does (internal/ui/render.go:12 Frame); `soak` runs it
//! with no terminal for a while, as `rush --soak` does
//! (cmd/rush/profile.go:46).

mod chores;
mod clanker;
pub mod config;
pub mod frame;
mod header;
pub mod hosted;
pub mod lead;
pub mod list;
mod model;
pub mod overlay;
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
    lead::hold_renewals(&m);
    setup(&mut m);
    // rush's colours for the Theme and Colours kept (applyColors).
    screens::settings::apply_colors(&mut m);
    // Links rush knows the kind of say where they stand (model.go:448).
    screens::cmdbar::linkkinds::start(&m);
    // Anything holding the UI thread over 40 ms is noted in stalls.log.
    frame::uistall::watch_ui(config::dir().join("stalls.log"));
    // Hosted leaves past conversations out of each reading until its
    // list is shown (NewHosted's loader.SkipPast(true)). Either way the
    // UI keeps the loader's handle, to link what a session spawned
    // (followSpawn's LinkSpawn).
    let tx = lp.sender().lane(1);
    let is_hosted = hosted.is_some();
    let give: Box<dyn FnOnce(worker::DiscoveryCtl) + Send> = Box::new(move |ctl| {
        let _ = tx.send(worker::Note::Apply(Box::new(move |m: &mut Model| {
            let f = m.kept::<worker::FleetIn>();
            f.0 = Some(ctl.handle());
            f.1 = Some(ctl.again());
            if is_hosted {
                hosted::set_discovery(m, ctl)
            } else {
                frame::Out::Idle
            }
        })));
    });
    worker::spawn_discovery_with(lp.sender(), is_hosted, Some(give));
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
        screens::settings::sheets::watch_net(&mut m);
        screens::settings::sheets::start_menu_bar(&mut m);
        screens::settings::updates::start(&mut m);
        screens::settings::sheets::start_view(&mut m);
    }
    let mut term = Term::new()?;
    m.keys_disambiguated = term.keys_disambiguated();
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

/// What `rush --soak` measured of the view's frames: how many reached the
/// screen changed, and the bytes of the rows that changed in them, as a
/// renderer redrawing changed rows writes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Soaked {
    pub writes: usize,
    pub bytes: usize,
}

/// `rush --soak D WxH`: the whole view run for d against the real agents
/// with no terminal, its frames drawn w×h in memory (cmd/rush/profile.go:46
/// soak). RUSH_RENDER_SELECT opens a Session beside the list once the list
/// is read, as a user would; RUSH_RENDER_KEYS are keys pressed in it a few
/// seconds later. setup gives the model what the binary does (run_with's).
pub fn soak(
    version: &str,
    d: std::time::Duration,
    w: u16,
    h: u16,
    setup: impl FnOnce(&mut Model),
) -> io::Result<Soaked> {
    use std::time::Duration;
    let lp: Loop<Note> = Loop::new();
    let mut m = Model::with_workers(version, lp.sender());
    m.cfg = config::Config::load();
    m.w = w as usize;
    m.h = h as usize;
    setup(&mut m);
    worker::spawn_discovery(lp.sender());
    let want = std::env::var("RUSH_RENDER_SELECT").unwrap_or_default();
    let keys = std::env::var("RUSH_RENDER_KEYS").unwrap_or_default();
    let tx = lp.sender().lane(4);
    std::thread::Builder::new()
        .name("rush-soak".into())
        .spawn(move || {
            let start = std::time::Instant::now();
            let at = |t: Duration| std::thread::sleep(t.saturating_sub(start.elapsed()));
            if !want.trim().is_empty() {
                // Picked once the list has been read (render.go:79 Select).
                at(Duration::from_secs(1).min(d));
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    let (w, h) = (m.w as u16, m.h as u16);
                    render_select(m, w, h, want.trim(), None);
                    frame::Out::Draw
                })));
            }
            if !keys.is_empty() {
                // Pressed once the Session has opened: "]" for its next tab.
                at(Duration::from_secs(3).min(d));
                tx.send(Note::Apply(Box::new(move |m: &mut Model| {
                    for c in keys.chars() {
                        m.key(Key::Char(c), KeyMods::NONE);
                    }
                    frame::Out::Draw
                })));
            }
            at(d);
            tx.send(Note::Apply(Box::new(|_| frame::Out::Quit)));
        })?;
    let mut hd = Headless::new(w, h);
    let mut was: Vec<String> = Vec::new();
    let mut out = Soaked::default();
    lp.run_with(&mut m, |m| {
        hd.draw(|c| rush_draw::App::<Note>::draw(m, c));
        let now: Vec<String> = hd.ansi().split('\n').map(str::to_string).collect();
        let changed: usize = now
            .iter()
            .enumerate()
            .filter(|(i, r)| was.get(*i) != Some(*r))
            .map(|(_, r)| r.len() + 1)
            .sum();
        if changed > 0 {
            out.writes += 1;
            out.bytes += changed;
        }
        was = now;
        Ok(())
    })?;
    worker::flush_spend();
    Ok(out)
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
/// RUSH_RENDER_SELECT, the agent whose name or id has it, opened. setup
/// gives the model what the binary does (its hosts, as run_with's), so
/// the agents an opened session hosted are found and read as the ticks
/// would (render.go:39 refreshSpawns, :40 refreshSubs).
pub fn render(
    version: &str,
    w: u16,
    h: u16,
    keys: &[(Key, KeyMods)],
    setup: impl FnOnce(&mut Model),
) -> String {
    let mut m = Model::new(version);
    m.cfg = config::Config::load();
    setup(&mut m);
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
    let want = std::env::var("RUSH_RENDER_SELECT").unwrap_or_default();
    let view = std::env::var("RUSH_RENDER_VIEW").ok();
    render_select(&mut m, w, h, &want, view.as_deref());
    frame(&mut m, w, h, keys, true)
}

/// --render's RUSH_RENDER_SELECT and RUSH_RENDER_VIEW: the agent whose
/// name or id has want opened, its runs and the agents it hosted read as
/// the ticks would, then its view named view shown (render.go:20-48).
pub fn render_select(m: &mut Model, w: u16, h: u16, want: &str, view: Option<&str>) {
    if want.is_empty() {
        return;
    }
    let want = want.to_lowercase();
    let Some(i) = m
        .agents
        .iter()
        .position(|a| a.display_name.to_lowercase().contains(&want) || a.job.id.starts_with(&want))
    else {
        return;
    };
    m.sel = m.agents[i].key.clone();
    m.w = w as usize;
    m.h = h as usize;
    m.open(i);
    // Let the pane connect, or read a stopped one's host info, before the
    // frame (render.go:29-35 syncHost).
    screens::session_core::live::sync(m);
    // Its runs and the agents its shell had rush host, found and read as
    // the ticks would (render.go:39 refreshSpawns, :40 refreshSubs), so
    // a subagents view is there to open and Changes counts their edits.
    if let Some(key) = screens::session_core::open_key(m) {
        screens::session_core::subs::refresh(m, &key);
    }
    // RUSH_RENDER_VIEW opens one of its views: overview, changes…
    // (Go's render.go:42).
    if let Some(v) = view
        && let Some(key) = screens::session_core::open_key(m)
    {
        screens::session_core::views::show_view(m, &key, v);
    }
    // Picked, not opened: beside the list when there's room for both,
    // the list keeping the keys (render.go:24 preview).
    if screens::list_extras::split::side_by_side(m) {
        screens::list_extras::split::leave_pane(m);
    }
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
