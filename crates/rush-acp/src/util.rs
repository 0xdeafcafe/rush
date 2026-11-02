//! Small helpers Go's standard library gives the adapters: filepath.Glob
//! over a folder's names, a file's modification time, the home folder.

use std::path::{Path, PathBuf};

use rush_core::time::Ms;

/// Go's filepath.Match for one name: `*` is any run of characters, `?` any
/// one; nothing else is special here.
pub(crate) fn matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Go's filepath.Glob of base joined with parts, each part a name or a
/// pattern of one: every path that is there, sorted.
pub(crate) fn glob(base: &Path, parts: &[&str]) -> Vec<PathBuf> {
    let mut found = vec![base.to_path_buf()];
    for part in parts {
        let wild = part.contains(['*', '?']);
        let mut next = Vec::new();
        for dir in &found {
            if !wild {
                let p = dir.join(part);
                if std::fs::symlink_metadata(&p).is_ok() {
                    next.push(p);
                }
                continue;
            }
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| matches(part, n))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| dir.join(n)));
        }
        found = next;
    }
    found
}

/// When a file was last written, in Unix milliseconds; 0 when it can't be
/// read.
pub(crate) fn mod_ms(m: &std::fs::Metadata) -> Ms {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as Ms)
}

/// The home folder (Go's os.UserHomeDir), empty when it isn't known.
pub(crate) fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// An environment variable, empty when it isn't set.
pub(crate) fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

/// Whether name is a program on PATH, or a path to one (Go's
/// exec.LookPath).
pub(crate) fn on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        return exe(Path::new(name));
    }
    env("PATH")
        .split(':')
        .filter(|d| Path::new(d).is_absolute())
        .any(|d| exe(&Path::new(d).join(name)))
}

/// p as a string.
pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Go's filepath.IsLocal: a relative path that stays inside its folder.
pub(crate) fn is_local(p: &str) -> bool {
    if p.is_empty() || Path::new(p).is_absolute() {
        return false;
    }
    let mut depth = 0i32;
    for c in Path::new(p).components() {
        match c {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            std::path::Component::Normal(_) => depth += 1,
            _ => {}
        }
    }
    true
}

/// The last element of a path, as Go's filepath.Base.
pub(crate) fn base(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { "." } else { "/" };
    }
    t.rsplit('/').next().unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_as_go_matches_them() {
        for (pat, name, want) in [
            ("wd_*", "wd_project_123", true),
            ("wd_*", "xwd_", false),
            ("session-*.json*", "session-1.jsonl", true),
            ("agent-*", "agent-", true),
            ("*", "", true),
            ("a?c", "abc", true),
            ("a*b*c", "aXbYbZc", true),
            ("a*b", "ab_", false),
        ] {
            assert_eq!(matches(pat, name), want, "{pat} {name}");
        }
        assert!(is_local("agents/x"));
        assert!(!is_local("../x"));
        assert!(!is_local("/x"));
        assert_eq!(base("/a/b/"), "b");
    }
}
