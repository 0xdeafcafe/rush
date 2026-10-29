//! rush's screens: the agent list and a session opened read-only, on
//! rush-draw's idle-free loop (DESIGN.md "Threads"). After Go's
//! internal/ui. `run` opens the TUI; `render` draws one frame, as Go's
//! `rush --render` does (internal/ui/render.go:12 Frame).

mod clanker;
pub mod config;
pub mod frame;
mod header;
pub mod list;
mod model;
pub mod screens;
mod session;
pub mod style;
pub mod worker;

use std::io;

use rush_core::time::now;
use rush_draw::{Headless, Key, KeyMods, Loop, Term};

pub use model::Model;
pub use worker::{Fleet, Note};

/// Opens the TUI and runs it until quit.
pub fn run(version: &str) -> io::Result<()> {
    run_with(version, |_| {})
}

/// Opens the TUI with what the binary gives it first (setup: the hosts
/// sessions run in, screens::session_core::host::set_hosts), and runs it
/// until quit.
pub fn run_with(version: &str, setup: impl FnOnce(&mut Model)) -> io::Result<()> {
    let lp: Loop<Note> = Loop::new();
    let mut m = Model::with_workers(version, lp.sender());
    m.cfg = config::Config::load();
    setup(&mut m);
    // Anything holding the UI thread over 40 ms is noted in stalls.log.
    frame::uistall::watch_ui(config::dir().join("stalls.log"));
    worker::spawn_discovery(lp.sender());
    // As Go's Init (internal/ui/model.go:548): plugins connected
    // (startHooks, pluginui.go:36), and the layout kept, or asked for the
    // first time (startView, model.go:574).
    screens::plugin_ui::start(&mut m, rush_plugind::hooks::Client::new());
    screens::settings::sheets::start_view(&mut m);
    let mut term = Term::new()?;
    let (w, h) = term.size()?;
    m.w = w as usize;
    m.h = h as usize;
    lp.read_input()?;
    let r = lp.run(&mut term, &mut m);
    // Quitting saves the spend read so far, so the next start needn't
    // read it all again (internal/ui/keys.go:644 quit: sc.Flush()).
    worker::flush_spend();
    r
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
    screens::accounts::state::load_read_only(&mut m);
    m.set_fleet(worker::load_once(now()));
    // What the ticks would have read by the first frame: the machine's
    // agent processes, and the launch folder's repository (Go's Frame).
    screens::projects_machine::read_machine(&mut m);
    screens::bars::root_launch_dir(&mut m);
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
            if let Ok(v) = std::env::var("RUSH_RENDER_VIEW")
                && let Some(key) = screens::session_core::open_key(&m)
            {
                screens::session_core::views::show_view(&mut m, &key, &v);
            }
        }
    }
    frame(&mut m, w, h, keys, true)
}

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
