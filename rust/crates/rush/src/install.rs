//! What the guide's install steps run: `rush on` makes `claude agents`
//! open rush (a line in the shell's rc that reads a hook rush writes),
//! `rush off` gives it back to Claude Code at once (a flag the hook reads
//! on each call), `rush status` says which, and `rush update` installs
//! the newest rush. After Go's cmd/rush/main.go (on :209 turnOn :391, off
//! :212, status :218, offFlag :349, isOn :351, hookPath :360, hookLine
//! :361, rcFile :363, hook :373, selfUpdate :578).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Where `rush on` writes: rush's folder and the shell's rc file.
pub struct Setup {
    pub dir: PathBuf,
    pub rc: PathBuf,
}

impl Setup {
    /// This user's: rush's folder, and ~/.bashrc under bash, else
    /// ~/.zshrc (rcFile).
    pub fn process() -> Setup {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_default();
        Setup {
            dir: rush_state::dir(),
            rc: rc_file(&home, &shell),
        }
    }

    /// The flag that turns the hook off (offFlag).
    pub fn off_flag(&self) -> PathBuf {
        self.dir.join("off")
    }

    /// The hook the rc file reads (hookPath).
    pub fn hook_path(&self) -> PathBuf {
        self.dir.join("shell.sh")
    }

    /// The line `rush on` adds to the rc file (hookLine).
    pub fn hook_line(&self) -> String {
        let p = self.hook_path();
        let p = p.display();
        format!("[ -f \"{p}\" ] && . \"{p}\"")
    }

    /// Whether `claude agents` opens rush: no off flag, and the rc file
    /// reads the hook (isOn).
    pub fn is_on(&self) -> bool {
        if self.off_flag().exists() {
            return false;
        }
        std::fs::read_to_string(&self.rc).is_ok_and(|b| b.contains(&self.hook_line()))
    }

    /// rush on: the hook written, the off flag gone, and the line added to
    /// the rc file if it isn't there yet (turnOn).
    pub fn turn_on(&self, out: &mut dyn Write) -> Result<(), String> {
        let r = self.turn_on_inner(out);
        let mut f = serde_json::Map::new();
        f.insert("rc".into(), self.rc.display().to_string().into());
        if let Err(e) = &r {
            f.insert("err".into(), e.clone().into());
        }
        rush_events::log("turned-on", f);
        r
    }

    fn turn_on_inner(&self, out: &mut dyn Write) -> Result<(), String> {
        mkdir_all(&self.dir)?;
        let hook = HOOK.replace("%s", &self.off_flag().display().to_string());
        write_file(&self.hook_path(), hook.as_bytes(), 0o644)?;
        let _ = std::fs::remove_file(self.off_flag());
        let line = self.hook_line();
        let b = std::fs::read_to_string(&self.rc).unwrap_or_default();
        if !b.contains(&line) {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .mode(0o644)
                .open(&self.rc)
                .map_err(|e| path_err("open", &self.rc, &e))?;
            f.write_all(format!("\n{line}\n").as_bytes())
                .map_err(|e| path_err("write", &self.rc, &e))?;
            let _ = writeln!(
                out,
                "on — added one line to {}. Open a new terminal (or: . {})",
                self.rc.display(),
                self.hook_path().display()
            );
            return Ok(());
        }
        let _ = writeln!(out, "on — \"claude agents\" opens rush");
        Ok(())
    }

    /// rush off: the flag the hook reads written, so it's instant
    /// (main.go:212).
    pub fn turn_off(&self, out: &mut dyn Write) -> Result<(), String> {
        let flag = self.off_flag();
        let r = write_file(&flag, b"", 0o600);
        let mut f = serde_json::Map::new();
        f.insert("flag".into(), flag.display().to_string().into());
        if let Err(e) = &r {
            f.insert("err".into(), e.clone().into());
        }
        rush_events::log("turned-off", f);
        r?;
        let _ = writeln!(
            out,
            "off — \"claude agents\" opens the native view. \"rush on\" turns it back on."
        );
        Ok(())
    }

    /// rush status (main.go:218).
    pub fn status(&self, out: &mut dyn Write) {
        let _ = if self.is_on() {
            writeln!(out, "on — \"claude agents\" opens rush")
        } else {
            writeln!(out, "off — \"claude agents\" opens the native view")
        };
    }
}

/// The rc file the shell reads: ~/.bashrc under bash, else ~/.zshrc
/// (rcFile).
pub fn rc_file(home: &Path, shell: &str) -> PathBuf {
    if shell.ends_with("bash") {
        home.join(".bashrc")
    } else {
        home.join(".zshrc")
    }
}

/// The hook only takes "claude agents"; every other claude call passes
/// through untouched, and the off flag is read per call so toggling is
/// instant (hook). %s is the off flag.
pub const HOOK: &str = r#"# added by "rush on" — remove with "rush off" or delete this file
claude() {
  if [ "$1" = "agents" ] && [ ! -f "%s" ] && command -v rush >/dev/null 2>&1; then
    shift
    command rush "$@"
  else
    command claude "$@"
  fi
}
"#;

/// An error on a path as Go's os package says it: op path: why.
fn path_err(op: &str, p: &Path, e: &std::io::Error) -> String {
    let why = match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => {
            let s = e.to_string();
            s.split(" (os error").next().unwrap_or(&s).to_lowercase()
        }
    };
    format!("{op} {}: {why}", p.display())
}

fn mkdir_all(d: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(d)
        .map_err(|e| path_err("mkdir", d, &e))
}

/// Go's os.WriteFile: made with perm if new, emptied, written.
fn write_file(p: &Path, b: &[u8], perm: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(perm)
        .open(p)
        .map_err(|e| path_err("open", p, &e))?;
    f.write_all(b).map_err(|e| path_err("write", p, &e))
}

/// What an update tells the rest of rush: the events log.
struct UpdateHost;

impl rush_update::Host for UpdateHost {
    fn log(&self, kind: &str, fields: serde_json::Value) {
        let f = match fields {
            serde_json::Value::Object(f) => f,
            _ => serde_json::Map::new(),
        };
        rush_events::log(kind, f);
    }
}

fn log1(kind: &str, pairs: &[(&str, String)]) {
    let mut f = serde_json::Map::new();
    for (k, v) in pairs {
        f.insert((*k).into(), v.clone().into());
    }
    rush_events::log(kind, f);
}

/// rush update: the newest rush over this one, when there is a newer one
/// (selfUpdate).
pub fn self_update(out: &mut dyn Write) -> Result<(), String> {
    let l = match rush_update::latest() {
        Ok(l) => l,
        Err(e) => {
            log1("update-failed", &[("err", e.0.clone())]);
            return Err(e.0);
        }
    };
    if let Some(cur) = rush_update::current()
        && !rush_update::newer(&cur, &l)
    {
        let _ = writeln!(out, "rush {} is the newest", cur.short());
        return Ok(());
    }
    let _ = writeln!(out, "installing rush {}…", l.short());
    let _ = out.flush();
    let to = match rush_update::install(&UpdateHost) {
        Ok(to) => to,
        Err(e) => {
            log1("update-failed", &[("to", l.short()), ("err", e.0.clone())]);
            return Err(e.0);
        }
    };
    log1("updated", &[("to", to.short())]);
    let _ = writeln!(out, "rush {} installed · reopen rush to use it", to.short());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(t: &tempfile::TempDir) -> Setup {
        Setup {
            dir: t.path().join("rush"),
            rc: t.path().join(".zshrc"),
        }
    }

    #[test]
    fn rc_by_shell() {
        let h = Path::new("/h");
        for (shell, want) in [
            ("/bin/bash", "/h/.bashrc"),
            ("/opt/homebrew/bin/bash", "/h/.bashrc"),
            ("/bin/zsh", "/h/.zshrc"),
            ("", "/h/.zshrc"),
            ("/usr/bin/fish", "/h/.zshrc"),
        ] {
            assert_eq!(rc_file(h, shell), PathBuf::from(want), "{shell}");
        }
    }

    #[test]
    fn on_adds_the_line_once() {
        let t = tempfile::tempdir().unwrap();
        let s = setup(&t);
        std::fs::write(&s.rc, "export A=1\n").unwrap();
        assert!(!s.is_on());
        let mut out = Vec::new();
        s.turn_on(&mut out).unwrap();
        let said = String::from_utf8(out).unwrap();
        assert_eq!(
            said,
            format!(
                "on — added one line to {}. Open a new terminal (or: . {})\n",
                s.rc.display(),
                s.hook_path().display()
            )
        );
        let rc = std::fs::read_to_string(&s.rc).unwrap();
        assert_eq!(rc, format!("export A=1\n\n{}\n", s.hook_line()));
        let hook = std::fs::read_to_string(s.hook_path()).unwrap();
        assert!(hook.contains(&format!("[ ! -f \"{}\" ]", s.off_flag().display())));
        assert!(hook.starts_with("# added by \"rush on\""));
        assert!(s.is_on());
        // Again: the line is there already.
        let mut out = Vec::new();
        s.turn_on(&mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "on — \"claude agents\" opens rush\n"
        );
        assert_eq!(std::fs::read_to_string(&s.rc).unwrap(), rc);
    }

    #[test]
    fn off_then_on_again() {
        let t = tempfile::tempdir().unwrap();
        let s = setup(&t);
        s.turn_on(&mut Vec::new()).unwrap();
        let mut out = Vec::new();
        s.turn_off(&mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "off — \"claude agents\" opens the native view. \"rush on\" turns it back on.\n"
        );
        assert!(s.off_flag().exists());
        assert!(!s.is_on());
        let mut out = Vec::new();
        s.status(&mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "off — \"claude agents\" opens the native view\n"
        );
        s.turn_on(&mut Vec::new()).unwrap();
        assert!(!s.off_flag().exists());
        let mut out = Vec::new();
        s.status(&mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "on — \"claude agents\" opens rush\n"
        );
    }

    #[test]
    fn off_without_rush_folder_fails() {
        let t = tempfile::tempdir().unwrap();
        let s = setup(&t);
        let e = s.turn_off(&mut Vec::new()).unwrap_err();
        assert_eq!(
            e,
            format!("open {}: no such file or directory", s.off_flag().display())
        );
    }
}
