//! Small helpers the sheets share, each after the Go helper it ports:
//! firstNonEmpty (rushmode.go:2934), approxTokens (skillsheet.go:224),
//! expand (keys.go:1128), plural (rewindsheet.go), cutRunes
//! (discuss.go), the tilde-less path join of the sheets.

use std::path::Path;

/// The first of xs that isn't empty (firstNonEmpty).
pub fn first_non_empty<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

/// A size in bytes as about how many tokens it is (approxTokens).
pub fn approx_tokens(size: i64) -> String {
    if size <= 0 {
        return String::new();
    }
    let t = size / 4;
    if t >= 1000 {
        format!("~{:.1}k tok", t as f64 / 1000.0)
    } else {
        format!("~{t} tok")
    }
}

/// p with ~ for home, made absolute (expand).
pub fn expand(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() {
        return String::new();
    }
    let mut p = p.to_string();
    if let Some(rest) = p.strip_prefix('~') {
        p = format!("{}{rest}", std::env::var("HOME").unwrap_or_default());
    }
    if !Path::new(&p).is_absolute()
        && let Ok(cwd) = std::env::current_dir()
    {
        p = cwd.join(&p).to_string_lossy().into_owned();
    }
    p
}

/// "s" unless n is one (plural).
pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// s cut to n characters, … after when it was cut (cutRunes).
pub fn cut_runes(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// dir joined with name (filepath.Join), name kept when it's absolute.
pub fn join(dir: &str, name: &str) -> String {
    Path::new(dir).join(name).to_string_lossy().into_owned()
}

/// A duration as Go's Duration.Round(time.Minute) prints it: 1h5m0s,
/// 12m0s, 0s.
pub fn go_minutes(d: rush_core::time::Ms) -> String {
    let m = (d + 30_000).max(0) / 60_000;
    match (m / 60, m % 60) {
        (0, 0) => "0s".into(),
        (0, m) => format!("{m}m0s"),
        (h, m) => format!("{h}h{m}m0s"),
    }
}
