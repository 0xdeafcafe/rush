//! A past conversation's title, folder and start, read from its
//! transcript's head and tail. Port of
//! internal/adapters/claude/claude/past.go.

use std::fs::File;
use std::path::Path;

use rush_core::time::Ms;
use serde::Deserialize;

use crate::json::{Str, contains, lines, ms, nd};
use crate::transcript::recent_events;

/// A conversation nothing has open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Convo {
    pub session_id: String,
    pub cwd: String,
    /// Its rename, or else the title Claude Code gave it, or else its first
    /// prompt.
    pub title: String,
    pub started: Ms,
}

/// Reads a transcript's start, for where it ran and what was asked first,
/// and its end, where Claude Code keeps appending its title. None for a
/// transcript with nothing asked in it (a hook's, a probe's), a background
/// job's (the jobs list has it), one a program ran through the SDK, or a
/// subagent's. buf is a reusable read buffer.
pub fn read_convo(path: &Path, buf: &mut Vec<u8>) -> Option<Convo> {
    let name = path.file_name()?.to_string_lossy();
    let mut c = Convo {
        session_id: name.strip_suffix(".jsonl").unwrap_or(&name).to_string(),
        ..Convo::default()
    };
    let f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let n = size.min(64 << 10) as usize;
    read_into(&f, buf, 0, n)?;
    let mut first = String::new();
    {
        let head = &buf[..];
        for b in lines(head) {
            // Once where and when are known, only a user's line, or one
            // that rules the transcript out, has more to say.
            if !c.cwd.is_empty()
                && c.started != 0
                && !contains(b, br#""type":"user""#)
                && !rules_out(b)
            {
                continue;
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct L<'a> {
                #[serde(rename = "type", borrow)]
                typ: Str<'a>,
                #[serde(borrow)]
                cwd: Str<'a>,
                #[serde(borrow)]
                timestamp: Str<'a>,
                #[serde(borrow)]
                entrypoint: Str<'a>,
                #[serde(rename = "isSidechain", deserialize_with = "nd")]
                sidechain: bool,
                #[serde(rename = "sessionKind", borrow)]
                kind: Str<'a>,
            }
            let Ok(l) = serde_json::from_slice::<L>(b) else {
                continue;
            };
            if l.sidechain || &*l.kind == "bg" || l.entrypoint.starts_with("sdk") {
                return None;
            }
            if c.cwd.is_empty() {
                c.cwd = l.cwd.to_string();
            }
            if c.started == 0 {
                c.started = ms(&l.timestamp);
            }
            if first.is_empty() && &*l.typ == "user" {
                for e in recent_events(std::iter::once(b), 1) {
                    if e.role == "user" {
                        first = e.text;
                    }
                }
            }
            if !c.cwd.is_empty() && c.started != 0 && !first.is_empty() {
                break;
            }
        }
    }
    if first.is_empty() || c.cwd.is_empty() {
        return None;
    }
    if size > 128 << 10 {
        read_into(&f, buf, size - (128 << 10), 128 << 10)?;
    } else if (n as u64) < size {
        read_into(&f, buf, 0, size as usize)?;
    }
    let (mut custom, mut ai) = (String::new(), String::new());
    for b in lines(buf) {
        if !b.starts_with(br#"{"type":"custom-title""#) && !b.starts_with(br#"{"type":"ai-title""#)
        {
            continue;
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct T<'a> {
            #[serde(rename = "customTitle", borrow)]
            custom: Str<'a>,
            #[serde(rename = "aiTitle", borrow)]
            ai: Str<'a>,
        }
        let Ok(l) = serde_json::from_slice::<T>(b) else {
            continue;
        };
        if !l.custom.is_empty() {
            custom = l.custom.into_owned();
        }
        if !l.ai.is_empty() {
            ai = l.ai.into_owned();
        }
    }
    c.title = if !custom.is_empty() {
        custom
    } else if !ai.is_empty() {
        ai
    } else {
        first
    };
    Some(c)
}

/// Fills buf with n bytes of f from off; fewer when the file ends.
fn read_into(f: &File, buf: &mut Vec<u8>, off: u64, n: usize) -> Option<()> {
    use std::os::unix::fs::FileExt;
    buf.clear();
    buf.resize(n, 0);
    let mut got = 0;
    while got < n {
        match f.read_at(&mut buf[got..], off + got as u64) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    buf.truncate(got);
    Some(())
}

/// Whether a line may mark a subagent's, a background job's or an SDK
/// program's transcript, without decoding it.
fn rules_out(b: &[u8]) -> bool {
    contains(b, br#""isSidechain":true"#)
        || contains(b, br#""sessionKind":"bg""#)
        || contains(b, br#""entrypoint":"sdk"#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tests::TempDir;

    #[test]
    fn read_convo_titles() {
        let d = TempDir::new("convo");
        let write = |name: &str, body: &str| {
            let p = d.0.join(format!("{name}.jsonl"));
            std::fs::write(&p, body).unwrap();
            p
        };
        let prompt = "{\"type\":\"user\",\"cwd\":\"/src/app\",\"timestamp\":\"2026-09-01T10:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"fix the build\"}}\n";
        let mut buf = Vec::new();
        let c = read_convo(&write("a1b2c3d4-first", prompt), &mut buf)
            .expect("a first prompt should title it");
        assert_eq!(c.title, "fix the build");
        assert_eq!(c.cwd, "/src/app");
        assert_eq!(c.session_id, "a1b2c3d4-first");
        assert_ne!(c.started, 0);
        let c = read_convo(
            &write(
                "ai",
                &format!("{prompt}{{\"type\":\"ai-title\",\"aiTitle\":\"Build fix\"}}\n"),
            ),
            &mut buf,
        )
        .unwrap();
        assert_eq!(
            c.title, "Build fix",
            "Claude Code's title should beat the prompt"
        );
        let c = read_convo(
            &write(
                "custom",
                &format!(
                    "{prompt}{{\"type\":\"custom-title\",\"customTitle\":\"mine\"}}\n{{\"type\":\"ai-title\",\"aiTitle\":\"Build fix\"}}\n"
                ),
            ),
            &mut buf,
        )
        .unwrap();
        assert_eq!(c.title, "mine", "a /rename should beat Claude Code's title");
        assert!(
            read_convo(
                &write(
                    "hooks",
                    "{\"type\":\"queue-operation\",\"timestamp\":\"2026-09-01T10:00:00Z\"}\n"
                ),
                &mut buf
            )
            .is_none(),
            "a transcript with nothing asked in it is no conversation"
        );
        let sdk = prompt.replace("\"cwd\"", "\"entrypoint\":\"sdk-cli\",\"cwd\"");
        assert!(
            read_convo(&write("sdk", &sdk), &mut buf).is_none(),
            "an SDK program's"
        );
        // A big one: the title is at the end, the prompt at the start.
        let mut big = prompt.to_string();
        let filler = format!(
            "{{\"type\":\"progress\",\"data\":\"{}\"}}\n",
            "x".repeat(1000)
        );
        for _ in 0..300 {
            big.push_str(&filler);
        }
        big.push_str("{\"type\":\"ai-title\",\"aiTitle\":\"Late\"}\n");
        let c = read_convo(&write("big", &big), &mut buf).unwrap();
        assert_eq!(c.title, "Late");
    }
}
