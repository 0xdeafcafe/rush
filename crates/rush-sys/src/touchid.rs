//! Touch ID for sudo on a Mac, turned on or off from Settings: macOS reads
//! /etc/pam.d/sudo_local (sudo's own file includes it, and a system
//! update leaves it be), and its pam_tid.so line, uncommented, has sudo
//! show the Touch ID screen instead of asking for a password. rush writes
//! that file from the template macOS ships, its pam_tid line uncommented
//! (turned_on), or the line commented out again (turned_off); never
//! /etc/pam.d/sudo itself. The file is root's, so it's written through
//! macOS's own administrator dialog (osascript's `with administrator
//! privileges`): rush never sees the password. Rust-only.

use std::path::Path;

/// The file sudo includes for local changes.
pub const LOCAL: &str = "/etc/pam.d/sudo_local";
/// What macOS ships to start it from.
pub const TEMPLATE: &str = "/etc/pam.d/sudo_local.template";

/// The line that turns Touch ID on, as the template has it.
const TID_LINE: &str = "auth       sufficient     pam_tid.so";

/// The template as macOS 14 ships it, for a Mac without one.
const SHIPPED: &str = "# sudo_local: local config file which survives system update and is included for sudo\n# uncomment following line to enable Touch ID for sudo\n#auth       sufficient     pam_tid.so\n";

/// Whether line (comment marks taken off first when commented) is an
/// `auth sufficient pam_tid.so` line.
fn tid_fields(line: &str) -> bool {
    let f: Vec<&str> = line.split_whitespace().collect();
    f.len() >= 3 && f[0] == "auth" && f[1] == "sufficient" && f[2] == "pam_tid.so"
}

/// line with its leading comment marks taken off, when it has any.
fn uncommented(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.starts_with('#').then(|| t.trim_start_matches('#'))
}

/// Whether sudo_local's text turns Touch ID on.
pub fn is_on(text: &str) -> bool {
    text.lines()
        .any(|l| !l.trim_start().starts_with('#') && tid_fields(l))
}

/// sudo_local's text with Touch ID on: current (the file as it is, when
/// it's there and not empty) or else the template, its commented pam_tid
/// line uncommented; with no such line, one put in before the first
/// rule that isn't pam_reattach's (which has to come before it). As it
/// was when it's on already.
pub fn turned_on(current: Option<&str>, template: Option<&str>) -> String {
    let base = current
        .filter(|s| !s.trim().is_empty())
        .or(template.filter(|s| !s.trim().is_empty()))
        .unwrap_or(SHIPPED);
    if is_on(base) {
        return base.to_string();
    }
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
    if let Some(i) = lines
        .iter()
        .position(|l| uncommented(l).is_some_and(tid_fields))
    {
        lines[i] = uncommented(&lines[i]).unwrap_or_default().to_string();
    } else {
        let at = lines
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#') && !t.contains("pam_reattach.so")
            })
            .unwrap_or(lines.len());
        lines.insert(at, TID_LINE.to_string());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// sudo_local's text with Touch ID off: each pam_tid line commented out,
/// the rest as it was.
pub fn turned_off(current: &str) -> String {
    let mut out: String = current
        .lines()
        .map(|l| {
            if !l.trim_start().starts_with('#') && tid_fields(l) {
                format!("#{}", l.trim_start())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// Whether this Mac can have Touch ID for sudo (it reads sudo_local: a
/// template or the file is there), and whether it's on now. None off a
/// Mac, or on one older than macOS 14.
pub fn read() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    match std::fs::read_to_string(LOCAL) {
        Ok(s) => Some(is_on(&s)),
        Err(_) if Path::new(TEMPLATE).exists() => Some(false),
        Err(_) => None,
    }
}

/// s as AppleScript reads a string: double quotes, backslash escapes.
fn applescript_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// s as the shell reads one word: single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The AppleScript that puts the file at from in place as sudo_local,
/// root's and read-only as macOS's own are, in macOS's administrator
/// dialog, saying why with prompt.
pub fn script(from: &Path, prompt: &str) -> String {
    let sh = format!(
        "/usr/bin/install -m 0444 -o root -g wheel {} {}",
        shell_quote(&from.to_string_lossy()),
        LOCAL
    );
    format!(
        "do shell script {} with prompt {} with administrator privileges",
        applescript_quote(&sh),
        applescript_quote(prompt)
    )
}

/// Turns Touch ID for sudo on or off: sudo_local written as turned_on or
/// turned_off says, through macOS's administrator dialog. Waits for the
/// dialog: off the UI thread. Err says why it isn't changed (you
/// cancelled it, or what went wrong).
pub fn set(on: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Touch ID for sudo is a Mac's".into());
    }
    if rush_core::sandbox::active() {
        return Err("a test never changes /etc".into());
    }
    let current = std::fs::read_to_string(LOCAL).ok();
    if current.as_deref().is_some_and(is_on) == on {
        return Ok(());
    }
    let text = if on {
        let template = std::fs::read_to_string(TEMPLATE).ok();
        turned_on(current.as_deref(), template.as_deref())
    } else {
        turned_off(current.as_deref().unwrap_or_default())
    };
    let dir = tempfile_dir()?;
    let from = dir.join("sudo_local");
    std::fs::write(&from, text).map_err(|e| e.to_string())?;
    let prompt = if on {
        "rush wants to turn on Touch ID for sudo."
    } else {
        "rush wants to turn off Touch ID for sudo."
    };
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script(&from, prompt)])
        .stdin(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("-128") {
        return Err("you cancelled it".into());
    }
    Err(err.trim().to_string())
}

/// A folder of rush's own to write the new file in first.
fn tempfile_dir() -> Result<std::path::PathBuf, String> {
    let d = std::env::temp_dir().join(format!("rush-touchid-{}", std::process::id()));
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE_TEXT: &str = "# sudo_local: local config file which survives system update and is included for sudo\n# uncomment following line to enable Touch ID for sudo\n#auth       sufficient     pam_tid.so\n";

    /// Touch ID turned on from the template, from a file already there,
    /// and from nothing; on already, it's left as it was.
    #[test]
    fn turned_on_uncomments_the_templates_line() {
        let on = "# sudo_local: local config file which survives system update and is included for sudo\n# uncomment following line to enable Touch ID for sudo\nauth       sufficient     pam_tid.so\n";
        let cases: &[(&str, Option<&str>, Option<&str>, &str)] = &[
            ("the template", None, Some(TEMPLATE_TEXT), on),
            ("an empty file", Some(""), Some(TEMPLATE_TEXT), on),
            ("no template", None, None, on),
            ("the file, commented", Some(TEMPLATE_TEXT), None, on),
            ("on already", Some(on), Some(TEMPLATE_TEXT), on),
            (
                "pam_reattach first",
                Some("# mine\nauth       optional       /opt/homebrew/lib/pam/pam_reattach.so\n"),
                None,
                "# mine\nauth       optional       /opt/homebrew/lib/pam/pam_reattach.so\nauth       sufficient     pam_tid.so\n",
            ),
            (
                "before another rule",
                Some("auth       sufficient     pam_smartcard.so\n"),
                None,
                "auth       sufficient     pam_tid.so\nauth       sufficient     pam_smartcard.so\n",
            ),
            (
                "## and spaces",
                Some("  ## auth sufficient pam_tid.so\n"),
                None,
                " auth sufficient pam_tid.so\n",
            ),
        ];
        for (name, current, template, want) in cases {
            let got = turned_on(*current, *template);
            assert_eq!(got, *want, "{name}");
            assert!(is_on(&got), "{name}");
        }
    }

    /// Off comments the line out again, the rest left; the template is
    /// off, and a mention elsewhere isn't on.
    #[test]
    fn turned_off_and_is_on() {
        let on = turned_on(None, Some(TEMPLATE_TEXT));
        assert_eq!(turned_off(&on), TEMPLATE_TEXT);
        let cases = [
            (TEMPLATE_TEXT, false),
            ("auth sufficient pam_tid.so", true),
            ("auth\tsufficient\tpam_tid.so # Touch ID", true),
            ("auth required pam_tid.so", false),
            ("# auth sufficient pam_tid.so", false),
            ("", false),
        ];
        for (text, want) in cases {
            assert_eq!(is_on(text), want, "{text:?}");
        }
    }

    /// The script installs the file as root's, quoted for the shell and
    /// for AppleScript, and asks in macOS's administrator dialog.
    #[test]
    fn the_script_installs_through_the_admin_dialog() {
        let s = script(Path::new("/tmp/it's \"here\"/sudo_local"), "rush wants it.");
        assert_eq!(
            s,
            r#"do shell script "/usr/bin/install -m 0444 -o root -g wheel '/tmp/it'\\''s \"here\"/sudo_local' /etc/pam.d/sudo_local" with prompt "rush wants it." with administrator privileges"#
        );
        assert!(!s.contains("/etc/pam.d/sudo "), "never sudo itself");
    }
}
