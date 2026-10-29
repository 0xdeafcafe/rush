//! #ghostty: LangWatch's light and dark themes for Ghostty, following the
//! system's appearance. The themes go in Ghostty's themes folder and its
//! config gets one theme line; colours the config set itself are
//! commented out (they'd beat the theme) and come back with #ghostty off.
//! After Go's internal/ui/ghostty.go (ghosttyDir :27, ghosttyConfig :39,
//! ghosttyColourKey :55, ghosttyOn :70, ghosttyOff :89, ghosttyCommand
//! :140).

use std::io;
use std::path::PathBuf;

use super::Out;
use crate::model::Model;

const DARK_THEME: &str = include_str!("ghosttythemes/langwatch-dark");
const LIGHT_THEME: &str = include_str!("ghosttythemes/langwatch-light");

/// The line rush puts in Ghostty's config (which takes no comment after
/// a value), known again by being exactly this.
pub const THEME: &str = "theme = light:langwatch-light,dark:langwatch-dark";
/// Marks a config line rush commented out, to put back.
pub const HELD: &str = "# rush #ghostty held: ";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Ghostty's XDG folder, where it looks for themes.
pub fn dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        Some(x) => PathBuf::from(x).join("ghostty"),
        None => home().join(".config").join("ghostty"),
    }
}

/// The config file Ghostty reads that rush edits: the first that exists,
/// the XDG one if none does.
pub fn config() -> PathBuf {
    let app = home()
        .join("Library")
        .join("Application Support")
        .join("com.mitchellh.ghostty");
    let d = dir();
    [
        app.join("config.ghostty"),
        app.join("config"),
        d.join("config.ghostty"),
        d.join("config"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .unwrap_or_else(|| d.join("config.ghostty"))
}

/// Whether a config line sets a colour the theme sets.
pub fn colour_key(line: &str) -> bool {
    let Some((k, _)) = line.split_once('=') else {
        return false;
    };
    matches!(
        k.trim(),
        "theme"
            | "background"
            | "foreground"
            | "palette"
            | "cursor-color"
            | "cursor-text"
            | "selection-background"
            | "selection-foreground"
    )
}

/// config with rush's themes in: colours it set held as comments, the
/// theme line at the end (ghosttyOn).
pub fn on(config: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in config.trim_end_matches('\n').split('\n') {
        if l == THEME {
            continue;
        }
        if colour_key(l) {
            out.push(format!("{HELD}{l}"));
        } else {
            out.push(l.to_string());
        }
    }
    if out.len() == 1 && out[0].is_empty() {
        out.clear();
    }
    out.push(THEME.to_string());
    out.join("\n") + "\n"
}

/// config with rush's line out and what it held back (ghosttyOff).
pub fn off(config: &str) -> String {
    config
        .split('\n')
        .filter(|l| *l != THEME)
        .map(|l| l.strip_prefix(HELD).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn install() -> io::Result<()> {
    let themes = dir().join("themes");
    std::fs::create_dir_all(&themes)?;
    std::fs::write(themes.join("langwatch-dark"), DARK_THEME)?;
    std::fs::write(themes.join("langwatch-light"), LIGHT_THEME)?;
    let path = config();
    let b = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&path, on(&b))
}

pub fn remove() -> io::Result<()> {
    let path = config();
    if let Ok(b) = std::fs::read_to_string(&path) {
        std::fs::write(&path, off(&b))?;
    }
    for name in ["langwatch-dark", "langwatch-light"] {
        match std::fs::remove_file(dir().join("themes").join(name)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

pub fn is_on() -> bool {
    std::fs::read_to_string(config()).is_ok_and(|b| b.contains(THEME))
}

/// #ghostty [on|off] (ghosttyCommand). The files are read and written off
/// the UI thread.
pub fn command(m: &mut Model, arg: &str) -> Out {
    match arg.trim() {
        "on" => m.later(install, |m, r| {
            match r {
                Ok(()) => m.flash(
                    "LangWatch themes on in Ghostty, light and dark following the system · ⌘⇧, in Ghostty loads them",
                ),
                Err(e) => m.flash_err(&e.to_string()),
            }
            Out::Draw
        }),
        "off" => m.later(remove, |m, r| {
            match r {
                Ok(()) => m.flash("LangWatch themes off · Ghostty's own colours are back · ⌘⇧, in Ghostty loads them"),
                Err(e) => m.flash_err(&e.to_string()),
            }
            Out::Draw
        }),
        _ => m.later(is_on, |m, on| {
            if on {
                m.flash("Ghostty is on LangWatch's themes · #ghostty off");
            } else {
                m.flash("#ghostty on puts Ghostty on LangWatch's light and dark themes, following the system");
            }
            Out::Draw
        }),
    }
    Out::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_and_off_round_trip() {
        let cfg = "font-size = 13\nbackground = #000000\npalette = 0=#111111\n";
        let o = on(cfg);
        assert_eq!(
            o,
            format!(
                "font-size = 13\n{HELD}background = #000000\n{HELD}palette = 0=#111111\n{THEME}\n"
            )
        );
        assert_eq!(on(&o), o, "on twice is on once");
        assert_eq!(off(&o), cfg);
        assert_eq!(on(""), format!("{THEME}\n"));
        assert!(!colour_key("font-size = 13"));
        assert!(colour_key(" cursor-color= #fff"));
    }
}
