//! Reading and comparing versions.
//!
//! Ported from internal/harnessup/version.go:1-53.

/// The version in out, or empty when none looks like one: the first
/// version-looking token of a program's --version, as "2.1.284 (Claude
/// Code)", "codex-cli 0.155.1", "v1.0.3-beta.2". (Go's regexp
/// `v?(\d+\.\d+(?:\.\d+)*(?:-[0-9A-Za-z][0-9A-Za-z.]*)?)`.)
pub fn parse(out: &str) -> String {
    let b = out.as_bytes();
    let digits = |i: usize| b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let n = digits(i);
        let mut j = i + n;
        // \.\d+ at least once, then as many more as there are.
        let mut parts = 0;
        while j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
            j += 1 + digits(j + 1);
            parts += 1;
        }
        if parts == 0 {
            i += n; // every later start in this run fails the same way
            continue;
        }
        if j + 1 < b.len() && b[j] == b'-' && b[j + 1].is_ascii_alphanumeric() {
            j += 2;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.') {
                j += 1;
            }
        }
        return out[i..j].to_string();
    }
    String::new()
}

/// Whether latest is a later version than installed. Numbers go by value
/// (0.9 is before 0.10); at equal numbers a pre-release ("-beta") is
/// before the release. Either empty is never newer.
pub fn newer(latest: &str, installed: &str) -> bool {
    if latest.is_empty() || installed.is_empty() {
        return false;
    }
    let (lv, lpre) = latest.split_once('-').unwrap_or((latest, ""));
    let (iv, ipre) = installed.split_once('-').unwrap_or((installed, ""));
    let (ln, inn) = (numbers(lv), numbers(iv));
    for i in 0..ln.len().max(inn.len()) {
        let l = ln.get(i).copied().unwrap_or(0);
        let c = inn.get(i).copied().unwrap_or(0);
        if l != c {
            return l > c;
        }
    }
    !ipre.is_empty() && lpre.is_empty()
}

fn numbers(v: &str) -> Vec<i64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        for (inp, want) in [
            ("2.1.284 (Claude Code)", "2.1.284"),
            ("codex-cli 0.155.1\n", "0.155.1"),
            ("GitHub Copilot CLI 1.0.3.", "1.0.3"),
            ("v1.0.3-beta.2", "1.0.3-beta.2"),
            ("ollama version is 0.12.3", "0.12.3"),
            ("no version here", ""),
            ("node v22 needed, have 0.3.4", "0.3.4"),
            ("1.0-", "1.0"),
            ("1.2-rc.1. done", "1.2-rc.1."),
        ] {
            assert_eq!(parse(inp), want, "parse({inp:?})");
        }
    }

    #[test]
    fn newer_versions() {
        for (latest, installed, want) in [
            ("2.1.285", "2.1.284", true),
            ("0.10.0", "0.9.9", true), // by value, not as text
            ("1.0", "1.0.0", false),
            ("1.0.0", "1.0.0-beta.1", true),
            ("1.0.0-beta.1", "1.0.0", false),
            ("1.0.0", "1.0.1", false),
            ("", "1.0.0", false),
            ("1.0.0", "", false),
        ] {
            assert_eq!(
                newer(latest, installed),
                want,
                "newer({latest:?}, {installed:?})"
            );
        }
    }
}
