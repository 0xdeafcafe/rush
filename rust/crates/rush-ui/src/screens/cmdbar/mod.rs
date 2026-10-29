//! ctrl+k, go anywhere: the command bar and its transcript search, the
//! keys guide (F1), the keymap in force and remapping keys through it, the
//! drawn keyboard, the link menu, and the # commands' table (Go:
//! cmdbar.go, keyhelp.go, keybind.go, keyboard.go, linkmenu.go,
//! fleetslash.go, newcmd.go, oncmd.go, known.go, offui.go, outside.go).

pub mod bar;
pub mod draw;
pub mod fleetslash;
pub mod fuzzy;
pub mod keybind;
pub mod keyboard;
pub mod keyhelp;
pub mod known;
pub mod linkmenu;
pub mod offui;
pub mod oncmd;
pub mod search;

use rush_core::text::Line;
use rush_draw::{Button, MouseKind};

use crate::frame::{Ext, Mouse, Out, Press};
use crate::model::Model;

/// Adds this group's keys, hooks and commands to x (frame/ext.rs), in
/// Go's order (keys.go:36-55): the keymap first, then the keys guide,
/// then the command bar opening and taking keys.
pub fn register(x: &mut Ext) {
    x.keys.push(keybind::remap);
    x.keys.push(keyhelp::key);
    x.keys.push(bar_keys);
    x.list_keys.push(list_question);
    x.mouse.push(mouse);
    x.overlays.push(overlay);
    x.session_dock.push(jump_dock);
    x.command("help", "every key rush answers to (F1)", |m, _| {
        keyhelp::open(m);
        Out::Draw
    });
    x.command(
        "on",
        "start an agent on another machine's rush",
        oncmd::on_command,
    );
}

/// ctrl+k, cmd+k and ctrl+f open the bar or change its scope; while it's
/// open it has every key but ctrl+q (keys.go:51-55).
fn bar_keys(m: &mut Model, k: &Press) -> Out {
    let o = bar::toggle(m, &k.s);
    if o.used() {
        return o;
    }
    if bar::is_open(m) && k.s != "ctrl+q" {
        return bar::key(m, k).or_idle();
    }
    Out::Pass
}

/// ? in the list with nothing typed opens the keys guide (keys.go:505).
fn list_question(m: &mut Model, k: &Press) -> Out {
    if k.s == "?" && m.input.is_empty() {
        keyhelp::open(m);
        return Out::Draw;
    }
    Out::Pass
}

/// A jump into a conversation from the bar finishes once the conversation
/// has read as far as the match: tried each time the open conversation
/// draws, before its body is laid out (applyJump, called as the pane
/// reads). It adds no rows.
fn jump_dock(m: &mut Model, _w: usize) -> Vec<Line> {
    if m.kept
        .peek::<bar::BarState>()
        .is_some_and(|s| s.jump.is_some())
    {
        bar::apply_jump(m);
    }
    Vec::new()
}

/// The guide or the bar over the screen; the guide over the bar
/// (view.go:620).
fn overlay(m: &mut Model, out: &mut Vec<Line>) {
    if keyhelp::is_open(m) {
        keyhelp::overlay(m, out);
    } else if bar::is_open(m) {
        draw::overlay(m, out);
    }
}

/// The mouse over the guide or the bar: a click on a row picks it, the
/// wheel moves the cursor, a click outside puts it away (barMsg's mouse,
/// overlayclick.go:42-44).
fn mouse(m: &mut Model, e: &Mouse) -> Out {
    let help = keyhelp::is_open(m);
    if !help && !bar::is_open(m) {
        return Out::Pass;
    }
    let o = m.over;
    let (x, y) = (e.x as usize, e.y as usize);
    let inside = o.on && x >= o.x0 && x < o.x1 && y >= o.y0 && y < o.y1;
    let row = y.checked_sub(o.by);
    match e.kind {
        MouseKind::ScrollUp | MouseKind::ScrollDown => {
            let d = if e.kind == MouseKind::ScrollUp { -3 } else { 3 };
            if help {
                if let Some(h) = m.kept::<keyhelp::HelpState>().open.as_mut() {
                    (0..3).for_each(|_| h.step(d / 3));
                }
            } else if let Some(b) = m.kept::<bar::BarState>().bar.as_mut()
                && !b.items.is_empty()
            {
                b.cursor = (b.cursor as isize + d).clamp(0, b.items.len() as isize - 1) as usize;
            }
            Out::Draw
        }
        MouseKind::Down(Button::Left) => {
            if !inside {
                if help {
                    keyhelp::close(m);
                } else {
                    bar::close(m);
                }
                return Out::Draw;
            }
            let Some(r) = row else { return Out::Idle };
            if help {
                let h = m.kept::<keyhelp::HelpState>().open.as_mut().expect("open");
                if let Some(&(_, i)) = h.rows_at.iter().find(|(l, _)| *l == r) {
                    if h.cur == i {
                        return keyhelp::key(m, &Press::of("enter").expect("enter"));
                    }
                    h.cur = i;
                }
                return Out::Draw;
            }
            let b = m.kept::<bar::BarState>().bar.as_mut().expect("open");
            if let Some(&(_, i)) = b.rows_at.iter().find(|(l, _)| *l == r) {
                b.cursor = i;
                return bar::key(m, &Press::of("enter").expect("enter"));
            }
            Out::Idle
        }
        _ => Out::Idle,
    }
}
