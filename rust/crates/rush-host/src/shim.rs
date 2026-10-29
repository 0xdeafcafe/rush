//! A session rush starts finds its stand-ins first on PATH: a codex or
//! claude its shell runs is `rush spawn codex …`, which hosts the run so
//! you can talk to it while it works, or runs the real program when it
//! can't.
//!
//! Ported from internal/host/shim.go.

use std::path::{Path, PathBuf};

use rush_core::adapter::Registry;

/// The folder of stand-ins.
pub fn shim_dir() -> PathBuf {
    rush_state::cache_path("shims")
}

/// The stand-in for program: rush spawn, or the real one off PATH when
/// this rush has gone.
pub fn shim_script(exe: &str, program: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\n# rush's stand-in: rush hosts the run when it can.\na={}\n[ -x \"$a\" ] && exec \"$a\" spawn {} \"$@\"\np=; IFS=:; for x in $PATH; do [ \"$x\" = {} ] || p=\"$p${{p:+:}}$x\"; done; unset IFS\nPATH=$p; export PATH\nexec {} \"$@\"\n",
        quote(exe),
        quote(program),
        quote(&shim_dir().to_string_lossy()),
        quote(program),
    )
    .into_bytes()
}

/// Puts a stand-in for every registered agent's program in shim_dir, and
/// is the folder, or None when it can't.
pub fn write_shims(reg: &Registry) -> Option<PathBuf> {
    let d = shim_dir();
    let exe = rush_gate::plugin::exe_for(&d); // never a scratch build: every agent here runs them
    if exe.is_empty() || mkdir(&d).is_err() {
        return None;
    }
    for a in reg.all().iter() {
        let p = reg.program_of(a.kind().as_str());
        if p.is_empty() {
            continue;
        }
        let path = d.join(&p);
        let want = shim_script(&exe, &p);
        if std::fs::read(&path).is_ok_and(|b| b == want) {
            continue;
        }
        let tmp = d.join(format!("{p}.tmp"));
        if write_mode(&tmp, &want, 0o700).is_err() || std::fs::rename(&tmp, &path).is_err() {
            return None;
        }
    }
    Some(d)
}

/// path with the stand-ins' folder, and the gate's, taken out.
pub fn without_shims(path: &str) -> String {
    let d = crate::tmp::clean(&shim_dir());
    let g = crate::tmp::clean(&rush_gate::bin_dir(&rush_gate::root()));
    path.split(':')
        .filter(|p| {
            !p.is_empty() && {
                let c = crate::tmp::clean(Path::new(p));
                c != d && c != g
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// What a session's agent gets so its shell finds the stand-ins: PATH with
/// them first, and which session it is. Claude Code runs each command in a
/// shell of your own profile's making, which sets PATH again: it sources
/// CLAUDE_ENV_FILE after, so that says it too.
pub fn shim_env(reg: &Registry, id: &str, kind: &str) -> Vec<String> {
    let mut env = vec![format!("RUSH_SESSION={id}")];
    let mut sh = String::new();
    // One you set is still read; another session's is its own.
    let root = crate::paths::root();
    if let Ok(own) = std::env::var("CLAUDE_ENV_FILE")
        && !own.is_empty()
        && !own.starts_with(&format!("{}/", root.to_string_lossy()))
    {
        sh.push_str(&format!("[ -f {} ] && . {}\n", quote(&own), quote(&own)));
    }
    sh.push_str(&format!("export RUSH_SESSION={}\n", quote(id)));
    if std::env::var("RUSH_NO_SHIM").unwrap_or_default().is_empty()
        && let Some(d) = write_shims(reg)
    {
        // The gate's shims come next: whichever agent it is, a queued
        // program it runs waits its turn.
        let groot = rush_gate::root();
        rush_gate::Plugin::new().write_shims(&groot);
        let g = rush_gate::bin_dir(&groot);
        let (ds, gs) = (d.to_string_lossy(), g.to_string_lossy());
        env.push(format!(
            "PATH={ds}:{gs}:{}",
            without_shims(&std::env::var("PATH").unwrap_or_default())
        ));
        sh.push_str(&format!("export PATH={}:{}\":$PATH\"\n", quote(&ds), quote(&gs)));
        // And every node process loads the gate's preload, for what
        // package scripts run from node_modules/.bin, past the shims.
        let cur = std::env::var("NODE_OPTIONS").unwrap_or_default();
        let no = rush_gate::plugin::node_options(&groot, &cur);
        if no != cur {
            env.push(format!("NODE_OPTIONS={no}"));
            let p = rush_gate::node::preload_path(&groot);
            let p = p.to_string_lossy();
            sh.push_str(&format!(
                "case \" $NODE_OPTIONS \" in *{}*) ;; *) export NODE_OPTIONS=\"--require {p} ${{NODE_OPTIONS:-}}\" ;; esac\n",
                quote(&p)
            ));
        }
        // Other agents run commands in a login zsh, whose profile puts PATH
        // in its own order: the stand-ins go first again after it.
        if let Some(z) = write_zsh(&d)
            && !reg.reads_as_claude(kind)
        {
            let home = std::env::var("HOME").unwrap_or_default();
            let zd = first(&[
                std::env::var("RUSH_ZDOTDIR").unwrap_or_default(),
                std::env::var("ZDOTDIR").unwrap_or_default(),
                home,
            ]);
            env.push(format!("RUSH_ZDOTDIR={zd}"));
            env.push(format!("ZDOTDIR={}", z.to_string_lossy()));
        }
    }
    // One Terraform plugin cache for every session: without it each scratch
    // checkout downloads the providers again, near a gigabyte each.
    if std::env::var("TF_PLUGIN_CACHE_DIR").unwrap_or_default().is_empty() {
        let tf = rush_state::cache_path("terraform-plugins");
        if mkdir(&tf).is_ok() {
            let tf = tf.to_string_lossy();
            env.push(format!("TF_PLUGIN_CACHE_DIR={tf}"));
            sh.push_str(&format!("export TF_PLUGIN_CACHE_DIR={}\n", quote(&tf)));
        }
    }
    let f = crate::paths::dir(id).join("shell-env.sh");
    if crate::json::write_private(&f, sh.as_bytes()).is_ok() {
        env.push(format!("CLAUDE_ENV_FILE={}", f.to_string_lossy()));
    }
    env
}

fn first(v: &[String]) -> String {
    v.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default()
}

/// The startup files zsh reads from ZDOTDIR.
const ZSH_FILES: [&str; 5] = [".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"];

/// Puts startup files for zsh in a folder beside the stand-ins, and is it,
/// or None when it can't: each reads your own (from where RUSH_ZDOTDIR
/// says, your home unless you moved them), and those read last in a shell
/// put the stand-ins first on PATH.
pub fn write_zsh(shims: &Path) -> Option<PathBuf> {
    let z = shims.join("zsh");
    mkdir(&z).ok()?;
    let g = rush_gate::bin_dir(&rush_gate::root());
    let (s, g) = (quote(&shims.to_string_lossy()), quote(&g.to_string_lossy()));
    let first = format!("path=({s} {g} ${{${{path:#{s}}}:#{g}}})\n");
    for f in ZSH_FILES {
        let last = if matches!(f, ".zshenv" | ".zshrc" | ".zlogin") {
            first.as_str()
        } else {
            ""
        };
        let body = format!(
            "# rush: your own {f}, read from where it is.\n_rush_z=$ZDOTDIR; ZDOTDIR=${{RUSH_ZDOTDIR:-$HOME}}\n[ -f \"$ZDOTDIR/{f}\" ] && . \"$ZDOTDIR/{f}\"\nexport RUSH_ZDOTDIR=$ZDOTDIR; ZDOTDIR=$_rush_z; unset _rush_z\n{last}"
        );
        let path = z.join(f);
        if std::fs::read(&path).is_ok_and(|b| b == body.as_bytes()) {
            continue;
        }
        let tmp = z.join(format!("{f}.tmp"));
        if write_mode(&tmp, body.as_bytes(), 0o600).is_err() || std::fs::rename(&tmp, &path).is_err() {
            return None;
        }
    }
    Some(z)
}

fn mkdir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p)
}

fn write_mode(p: &Path, b: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(p)?;
    f.write_all(b)?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))
}

/// s as one word to sh.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Home;

    #[test]
    fn writes_the_stand_in_as_go_does() {
        let _h = Home::new();
        let go = include_str!("../tests/fixtures/shim.sh");
        // Go's was written with its test's own cache: put this one's in.
        let line = go.lines().nth(4).unwrap();
        let start = line.find("= '").unwrap() + 3;
        let end = line[start..].find('\'').unwrap() + start;
        let want = go.replace(&line[start..end], &shim_dir().to_string_lossy());
        assert_eq!(String::from_utf8(shim_script("/usr/local/bin/rush's", "codex")).unwrap(), want);
    }

    // shim_test.go:12: a stand-in runs rush spawn, and the real program
    // when rush has gone: it never breaks the command.
    #[test]
    fn stand_in_runs_rush_or_the_real_program() {
        let h = Home::new();
        let cache = h.cache();
        let (shims, bin) = (shim_dir(), cache.join("bin"));
        for d in [&shims, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write = |p: &Path, body: &str| write_mode(p, format!("#!/bin/sh\n{body}").as_bytes(), 0o755).unwrap();
        write(&bin.join("codex"), "echo \"real $*\"\n");
        let rush = cache.join("rush");
        write(&rush, "echo \"rush $*\"\n");
        let run = || {
            let out = std::process::Command::new(shims.join("codex"))
                .args(["exec", "it's"])
                .env("PATH", format!("{}:{}:/usr/bin:/bin", shims.display(), bin.display()))
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        let script = |exe: &Path| String::from_utf8(shim_script(&exe.to_string_lossy(), "codex")).unwrap()["#!/bin/sh\n".len()..].to_string();
        write(&shims.join("codex"), &script(&rush));
        assert_eq!(run(), "rush spawn codex exec it's\n");
        write(&shims.join("codex"), &script(&cache.join("gone")));
        assert_eq!(run(), "real exec it's\n");
    }

    #[test]
    fn takes_the_stand_ins_off_path() {
        let _h = Home::new();
        let d = shim_dir().to_string_lossy().into_owned();
        assert_eq!(without_shims(&format!("/a:{d}:/b/:{d}/::/c")), "/a:/b/:/c");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }
}
