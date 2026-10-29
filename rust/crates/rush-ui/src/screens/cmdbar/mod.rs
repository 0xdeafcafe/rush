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

use crate::frame::Ext;

/// Adds this group's places, pages, sheets' openers, keys, hooks and
/// commands to x (frame/ext.rs).
pub fn register(_x: &mut Ext) {}
