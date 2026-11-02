//! Codex's HarnessSettings: the top-level keys of config.toml its
//! Harnesses page offers (model, reasoning effort, approvals, sandbox),
//! each with its value and where it comes from. Rush-only (Go planned
//! it). A change rewrites only that key's line: everything else in the
//! file, comments and tables included, stays byte for byte; the result
//! must still read as TOML before it's written, through a temp file
//! renamed onto the file (a link's target), readable by you alone.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rush_core::adapter::{OwnEdit, OwnGroup, OwnKind, OwnSetting, OwnSettings, Profile, ValueFrom};

/// The keys offered: key, label, kind, what, choices, how unset shows.
type Row = (
    &'static str,
    &'static str,
    OwnKind,
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

const ROWS: &[Row] = &[
    (
        "model",
        "Default model",
        OwnKind::Open,
        "The model every Codex session starts with unless rush or the session picks one (model).",
        &[("", "Codex's own default.")],
        "Codex's default",
    ),
    (
        "model_reasoning_effort",
        "Reasoning effort",
        OwnKind::Choice,
        "How hard sessions think by default (model_reasoning_effort). rush's own Effort for new sessions (Every harness, or Codex's row there) wins for the sessions rush starts.",
        &[
            ("", "the model's own default."),
            ("minimal", "the least thinking."),
            ("low", ""),
            ("medium", ""),
            ("high", ""),
            ("xhigh", "the most, on a model that has it."),
        ],
        "model's default",
    ),
    (
        "approval_policy",
        "Approvals",
        OwnKind::Choice,
        "When Codex asks before running a command (approval_policy). rush's Permissions for new sessions win for the sessions rush starts.",
        &[
            ("", "Codex's own default."),
            (
                "untrusted",
                "asks before any command that isn't known to be safe.",
            ),
            (
                "on-failure",
                "runs in the sandbox, and asks when a command fails there.",
            ),
            ("on-request", "the model asks when it thinks it should."),
            ("never", "never asks."),
        ],
        "Codex's default",
    ),
    (
        "sandbox_mode",
        "Sandbox",
        OwnKind::Choice,
        "What commands Codex runs may touch (sandbox_mode).",
        &[
            ("", "Codex's own default."),
            ("read-only", "they read anything and change nothing."),
            (
                "workspace-write",
                "they change the project and temp folders only.",
            ),
            (
                "danger-full-access",
                "no sandbox: they may change anything.",
            ),
        ],
        "Codex's default",
    ),
];

/// p's config.toml: its folder's, else ~/.codex's.
pub fn config_path(p: &Profile) -> PathBuf {
    let dir = if p.dir.is_empty() {
        Path::new(&std::env::var("HOME").unwrap_or_default()).join(".codex")
    } else {
        PathBuf::from(&p.dir)
    };
    dir.join("config.toml")
}

/// What the page shows of the config.toml at path; a missing one is
/// empty.
pub fn read(path: &Path) -> io::Result<OwnSettings> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let items = ROWS
        .iter()
        .map(|(key, label, kind, what, choices, unset)| {
            let value = match table.get(*key) {
                Some(toml::Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            OwnSetting {
                key: key.to_string(),
                label: label.to_string(),
                what: what.to_string(),
                kind: *kind,
                from: if value.is_empty() {
                    ValueFrom::Default
                } else {
                    ValueFrom::File
                },
                value,
                choices: choices
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
                unset: unset.to_string(),
            }
        })
        .collect();
    let shown = path.to_string_lossy().into_owned();
    Ok(OwnSettings {
        file: tilde(&shown),
        path: shown,
        note: "every Codex session reads it, not only rush's".into(),
        groups: vec![OwnGroup {
            title: "Codex's settings".into(),
            note: String::new(),
            items,
        }],
    })
}

/// Whether value can be key's: an offered key, and one of its choices
/// unless it's typed.
pub fn check(key: &str, value: &str) -> Result<(), String> {
    let Some((_, _, kind, _, choices, _)) = ROWS.iter().find(|r| r.0 == key) else {
        return Err(format!("rush doesn't change {key}"));
    };
    if *kind == OwnKind::Choice && !choices.iter().any(|(c, _)| *c == value) {
        let names: Vec<&str> = choices
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| !c.is_empty())
            .collect();
        return Err(format!("{key} is one of {}", names.join(", ")));
    }
    Ok(())
}

/// Writes edits into the config.toml at path, each key's line alone.
pub fn write(path: &Path, edits: &[OwnEdit]) -> io::Result<()> {
    let bad = |e: String| io::Error::new(io::ErrorKind::InvalidInput, e);
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = match fs::read_to_string(&real) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    for e in edits {
        check(&e.key, &e.value).map_err(bad)?;
        let lit = (!e.value.is_empty()).then(|| toml::Value::String(e.value.clone()).to_string());
        text = set_top_level(&text, &e.key, lit.as_deref());
    }
    toml::from_str::<toml::Table>(&text)
        .map_err(|e| bad(format!("the change wouldn't read as TOML: {e}")))?;
    let dir = real.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".config-{}.toml", std::process::id()));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, &real)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// text with top-level key set to the TOML literal lit (None removes
/// it): its line replaced where it has one before the first table, else
/// a line added after the last top-level key (at the top when there's
/// none). Nothing else changes.
pub fn set_top_level(text: &str, key: &str, lit: Option<&str>) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut at = None;
    let mut last_key = None;
    // A value over several lines: a multi-line string, or an array or
    // inline table not closed on its first line. A key's last line is
    // where one added after it goes.
    let mut in_multi: Option<&str> = None;
    let mut depth: i32 = 0;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if let Some(q) = in_multi {
            if t.matches(q).count() % 2 == 1 {
                in_multi = None;
                last_key = Some(i);
            }
            continue;
        }
        if depth > 0 {
            depth += brackets(t);
            if depth <= 0 {
                depth = 0;
                last_key = Some(i);
            }
            continue;
        }
        if t.starts_with('[') {
            break;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let k = k.trim().trim_matches('"').trim_matches('\'');
            if k == key {
                at = Some(i);
            }
            last_key = Some(i);
            for q in ["\"\"\"", "'''"] {
                if v.matches(q).count() % 2 == 1 {
                    in_multi = Some(q);
                }
            }
            if in_multi.is_none() {
                depth = brackets(v).max(0);
            }
        }
    }
    let new_line = lit.map(|v| format!("{key} = {v}\n"));
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (at, new_line) {
        (Some(i), Some(nl)) => {
            let indent: String = out[i].chars().take_while(|c| c.is_whitespace()).collect();
            let ends = out[i].ends_with('\n');
            out[i] = format!("{indent}{}", if ends { nl } else { nl.trim_end().into() });
        }
        (Some(i), None) => {
            out.remove(i);
        }
        (None, Some(nl)) => {
            let i = last_key.map_or(0, |i| i + 1);
            if i > 0 && !out[i - 1].ends_with('\n') {
                out[i - 1].push('\n');
            }
            out.insert(i, nl);
        }
        (None, None) => {}
    }
    out.concat()
}

/// How many more brackets and braces v opens than it closes, outside
/// its strings and comment.
fn brackets(v: &str) -> i32 {
    let (mut n, mut q) = (0, None);
    for c in v.chars() {
        match (q, c) {
            (None, '#') => break,
            (None, '"' | '\'') => q = Some(c),
            (Some(o), c) if c == o => q = None,
            (None, '[' | '{') => n += 1,
            (None, ']' | '}') => n -= 1,
            _ => {}
        }
    }
    n
}

/// path with the home folder as ~.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && path.starts_with(&h) => format!("~{}", &path[h.len()..]),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const FILE: &str = "# my settings\nmodel = \"gpt-5\"   # the usual\n\n[profiles.fast]\nmodel = \"o4-mini\"\napproval_policy = \"never\"\n\n[mcp_servers.x]\ncommand = \"npx\"\n";

    #[test]
    fn a_key_changes_alone() {
        let set = set_top_level(FILE, "model", Some("\"gpt-6\""));
        assert_eq!(
            set,
            FILE.replace("model = \"gpt-5\"   # the usual", "model = \"gpt-6\"")
        );
        let added = set_top_level(FILE, "sandbox_mode", Some("\"read-only\""));
        assert_eq!(
            added,
            FILE.replace(
                "# the usual\n",
                "# the usual\nsandbox_mode = \"read-only\"\n"
            ),
            "after the last top-level key, not in a table"
        );
        let gone = set_top_level(FILE, "model", None);
        assert!(
            !gone.contains("gpt-5") && gone.contains("o4-mini"),
            "{gone}"
        );
        assert_eq!(
            set_top_level(FILE, "approval_policy", None),
            FILE,
            "a key in a table isn't the top-level one"
        );
        assert_eq!(set_top_level("", "model", Some("\"m\"")), "model = \"m\"\n");
        let arr = "dirs = [\n  \"a\",\n]\n";
        assert_eq!(
            set_top_level(arr, "model", Some("\"y\"")),
            format!("{arr}model = \"y\"\n"),
            "after an array over several lines"
        );
        let multi = "notes = \"\"\"\nmodel = \"x\"\n\"\"\"\n";
        assert_eq!(
            set_top_level(multi, "model", Some("\"y\"")),
            format!("{multi}model = \"y\"\n"),
            "a line inside a multi-line string isn't a key"
        );
    }

    #[test]
    fn read_check_write() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.toml");
        fs::write(&real, FILE).unwrap();
        let home = d.path().join("codex");
        fs::create_dir_all(&home).unwrap();
        symlink(&real, home.join("config.toml")).unwrap();
        let path = home.join("config.toml");
        let o = read(&path).unwrap();
        let m = &o.groups[0].items[0];
        assert_eq!((m.value.as_str(), &m.from), ("gpt-5", &ValueFrom::File));
        let ap = o.groups[0]
            .items
            .iter()
            .find(|i| i.key == "approval_policy")
            .unwrap();
        assert_eq!(
            ap.from,
            ValueFrom::Default,
            "a table's key isn't the top level's"
        );

        assert!(check("approval_policy", "sometimes").is_err());
        assert!(check("model", "anything-typed").is_ok());
        assert!(check("unknown", "x").is_err());

        write(
            &path,
            &[
                OwnEdit::new("approval_policy", "on-request"),
                OwnEdit::new("model", ""),
            ],
        )
        .unwrap();
        let got = fs::read_to_string(&real).unwrap();
        assert!(got.starts_with("# my settings\n"), "{got}");
        assert!(
            got.contains("approval_policy = \"on-request\"\n\n[profiles.fast]"),
            "{got}"
        );
        assert!(
            got.contains("[mcp_servers.x]\ncommand = \"npx\"\n"),
            "{got}"
        );
        assert!(!got.contains("gpt-5"), "{got}");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write(&path, &[OwnEdit::new("sandbox_mode", "nope")]).is_err());
    }
}
