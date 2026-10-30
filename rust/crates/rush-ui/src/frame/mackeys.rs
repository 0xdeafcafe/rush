//! #mackeys: Terminal.app keeps ⌘ for its own menus, so ⌘← and the like
//! never reach rush there, and without "Use Option as Meta key" ⌥⌫
//! arrives as a plain backspace. Opted into, Hammerspoon sends them on as
//! the ctrl keys rush and the shell understand, only while Terminal.app
//! is in front. After Go's internal/ui/mackeys.go (inAppleTerminal :24,
//! hsScript :32, macKeysOn :93, macKeysCommand :103, macKeysInstall :150,
//! macKeysRemove :183, restartHammerspoon :202, lastLine :226).

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::Out;
use crate::model::Model;

/// Whether rush is drawing in macOS's own Terminal.app.
pub fn in_apple_terminal() -> bool {
    cfg!(target_os = "macos") && std::env::var("TERM_PROGRAM").is_ok_and(|t| t == "Apple_Terminal")
}

/// The line rush adds to Hammerspoon's init.lua.
pub const REQUIRE: &str =
    r#"require("rush") -- rush's ⌘ keys for Terminal.app; #mackeys off removes this"#;

/// ~/.hammerspoon/rush.lua.
pub const SCRIPT: &str = r#"-- Written by rush (#mackeys); #mackeys off removes it, and rush
-- overwrites any change. Terminal.app keeps ⌘ for its menus, so while it is
-- in front these ⌘ keys (and ⌥⌫) are sent as the ctrl keys a terminal
-- understands.
local M = {}

local remap = {
  { { "cmd" }, "left", { "ctrl" }, "a" },          -- line start
  { { "cmd" }, "right", { "ctrl" }, "e" },         -- line end
  { { "cmd" }, "delete", { "ctrl" }, "u" },        -- clear to line start
  { { "cmd" }, "forwarddelete", { "ctrl" }, "k" }, -- clear to line end (fn+⌘⌫)
  { { "cmd" }, "z", { "ctrl" }, "-" },             -- undo
  { { "cmd", "shift" }, "z", { "ctrl" }, "y" },    -- redo
  { { "alt" }, "delete", { "ctrl" }, "w" },        -- delete the word before
}

local codes = hs.keycodes.map

-- same is whether flags hold exactly mods; fn, set on every arrow, is ignored.
local function same(flags, mods)
  local want = {}
  for _, m in ipairs(mods) do want[m] = true end
  for _, m in ipairs({ "cmd", "alt", "ctrl", "shift" }) do
    if (flags[m] or false) ~= (want[m] or false) then return false end
  end
  return true
end

M.tap = hs.eventtap.new({ hs.eventtap.event.types.keyDown }, function(e)
  local code, flags = e:getKeyCode(), e:getFlags()
  for _, r in ipairs(remap) do
    if code == codes[r[2]] and same(flags, r[1]) then
      local app = hs.application.frontmostApplication()
      if not app or app:bundleID() ~= "com.apple.Terminal" then return false end
      return true, {
        hs.eventtap.event.newKeyEvent(r[3], r[4], true),
        hs.eventtap.event.newKeyEvent(r[3], r[4], false),
      }
    end
  end
  return false
end):start()

return M
"#;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn hs_dir() -> PathBuf {
    home().join(".hammerspoon")
}

/// Where Hammerspoon is installed, if it is.
pub fn hammerspoon_app() -> Option<PathBuf> {
    [
        PathBuf::from("/Applications/Hammerspoon.app"),
        home().join("Applications").join("Hammerspoon.app"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

/// Whether Hammerspoon is set to load rush's keys.
pub fn is_on() -> bool {
    std::fs::read_to_string(hs_dir().join("init.lua")).is_ok_and(|s| s.contains(REQUIRE))
        && hs_dir().join("rush.lua").exists()
}

/// init.lua's text with rush's line added once.
pub fn with_require(init: &str) -> String {
    if init.contains(REQUIRE) {
        return init.to_string();
    }
    let mut s = init.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s + REQUIRE + "\n"
}

/// init.lua's text with rush's line taken out.
pub fn without_require(init: &str) -> String {
    let s = init.replacen(&format!("{REQUIRE}\n"), "", 1);
    s.replacen(REQUIRE, "", 1)
}

fn err(s: String) -> io::Error {
    io::Error::other(s)
}

/// The last line a command printed, or its error.
pub fn last_line(out: &str, e: &str) -> String {
    let l = out.trim().rsplit('\n').next().unwrap_or("").trim();
    if l.is_empty() {
        e.to_string()
    } else {
        l.to_string()
    }
}

/// Installs Hammerspoon if it isn't, writes rush's keys into its config
/// and restarts it so they load.
pub fn install() -> io::Result<()> {
    if hammerspoon_app().is_none() {
        let out = Command::new("brew")
            .args(["install", "--cask", "hammerspoon"])
            .output()
            .map_err(|_| {
                err(
                    "#mackeys needs Hammerspoon: install it from hammerspoon.org, then #mackeys on"
                        .into(),
                )
            })?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            return Err(err(format!(
                "installing Hammerspoon: {}",
                last_line(&text, "brew failed")
            )));
        }
    }
    let dir = hs_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("rush.lua"), SCRIPT)?;
    let init = dir.join("init.lua");
    let had = match std::fs::read_to_string(&init) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if !had.contains(REQUIRE) {
        std::fs::write(&init, with_require(&had))?;
    }
    restart(true)
}

/// Takes rush's keys back out of Hammerspoon's config.
pub fn remove() -> io::Result<()> {
    let dir = hs_dir();
    let init = dir.join("init.lua");
    if let Ok(s) = std::fs::read_to_string(&init)
        && s.contains(REQUIRE)
    {
        std::fs::write(&init, without_require(&s))?;
    }
    match std::fs::remove_file(dir.join("rush.lua")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    restart(false)
}

fn running() -> bool {
    Command::new("pgrep")
        .args(["-xq", "Hammerspoon"])
        .status()
        .is_ok_and(|s| s.success())
}

/// Quits Hammerspoon if it's running, so it reads its config again, and
/// opens it in the background; start opens it even when it wasn't.
fn restart(start: bool) -> io::Result<()> {
    let was = running();
    if was {
        let _ = Command::new("osascript")
            .args(["-e", r#"tell application "Hammerspoon" to quit"#])
            .status();
        // Up to three seconds for it to go, as Go's 30 × 100 ms.
        for _ in 0..30 {
            if !running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if !was && !start {
        return Ok(());
    }
    let app = hammerspoon_app().unwrap_or_else(|| PathBuf::from("/Applications/Hammerspoon.app"));
    let out = Command::new("open").arg("-g").arg(&app).output()?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(err(format!(
            "opening Hammerspoon: {}",
            last_line(&text, "open failed")
        )));
    }
    Ok(())
}

/// #mackeys [on|off] (macKeysCommand). All of it runs off the UI thread.
pub fn command(m: &mut Model, arg: &str) -> Out {
    if !cfg!(target_os = "macos") {
        m.flash_err("#mackeys is for macOS's Terminal.app");
        return Out::Draw;
    }
    match arg.trim() {
        "on" => {
            m.flash("setting up ⌘ keys for Terminal.app…");
            // Whether Hammerspoon is there is looked at off the UI thread;
            // a brew install says so first, as it takes a while.
            m.later(hammerspoon_app, |m, app| {
                if app.is_none() {
                    m.flash("installing Hammerspoon with brew…");
                }
                m.later(install, |m, r| {
                    match r {
                        Ok(()) => m.flash(
                            "⌘ keys on in Terminal.app: ⌘← → line ends · ⌘⌫ ⌘⌦ clear to them · ⌥⌫ a word · ⌘Z undo · allow Hammerspoon in Accessibility if macOS asks",
                        ),
                        Err(e) => m.fail(&e.to_string()),
                    }
                    Out::Draw
                });
                Out::Draw
            });
            Out::Draw
        }
        "off" => {
            m.later(remove, |m, r| {
                match r {
                    Ok(()) => m.flash("⌘ keys off · Hammerspoon stays installed"),
                    Err(e) => m.fail(&e.to_string()),
                }
                Out::Draw
            });
            Out::Idle
        }
        _ => {
            m.later(is_on, |m, on| {
                if on {
                    m.flash("⌘ keys are on in Terminal.app, through Hammerspoon · #mackeys off");
                } else {
                    m.flash(
                        "⌘ keys are off · #mackeys on sends ⌘← → ⌘⌫ ⌘⌦ ⌘Z on through Hammerspoon",
                    );
                }
                Out::Draw
            });
            Out::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_line_in_and_out() {
        assert_eq!(with_require(""), format!("{REQUIRE}\n"));
        let init = "hs.alert('hi')";
        let with = with_require(init);
        assert_eq!(with, format!("hs.alert('hi')\n{REQUIRE}\n"));
        assert_eq!(with_require(&with), with);
        assert_eq!(without_require(&with), "hs.alert('hi')\n");
        assert_eq!(last_line("a\nb\n\n", "e"), "b");
        assert_eq!(last_line("  ", "e"), "e");
    }
}
