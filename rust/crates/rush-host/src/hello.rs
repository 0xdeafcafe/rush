//! A client says hello first, with its protocol (op "hello", its proto).
//! One from EVENTS_FROM on gets a Claude Code session as rush's own
//! events, as any other agent's session comes; one that says less, or
//! sends no hello (an older rush), gets Claude Code's lines as it always
//! has. Hosts from before the hello take it for an op they don't know, and
//! do nothing.
//!
//! Ported from internal/host/hello.go.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use rush_core::adapter::Neutral;

use crate::lines::{Line, decode, event_line, is_claude_line, native_line, neutral};
use crate::op::Op;

/// The first client protocol that reads Claude Code's sessions as rush's
/// own events.
pub const EVENTS_FROM: i64 = 6;

/// How long a new connection's first line may take to say what the client
/// reads, before it's taken for an older rush's.
pub const HELLO_WAIT: Duration = Duration::from_millis(300);

/// A connection's first line, waited for up to HELLO_WAIT: the protocol
/// the client says it speaks, or 0; what's left to read of the connection
/// as ops; and the first line (or what came of it) when that wasn't a
/// hello, to be read as an op first.
pub fn hello(nc: &UnixStream) -> std::io::Result<(i64, BufReader<UnixStream>, Vec<u8>)> {
    let mut r = BufReader::with_capacity(64 << 10, nc.try_clone()?);
    // Errors setting the wait are let be, as Go's are (hello.go:34): on
    // macOS they come from a client that has already gone, whose ops are
    // still there to read.
    let _ = nc.set_read_timeout(Some(HELLO_WAIT));
    let mut line = Vec::new();
    let got = r.read_until(b'\n', &mut line);
    let _ = nc.set_read_timeout(None);
    if matches!(got, Ok(n) if n > 0 && line.ends_with(b"\n"))
        && let Ok(o) = Op::parse(&line)
        && o.op == "hello"
    {
        return Ok((o.proto, r, Vec::new()));
    }
    // No hello: what came is the start of the ops, read again from the top.
    Ok((0, r, line))
}

/// Turns Claude Code's lines into rush's own events, for one client that
/// reads them. Everything else, the host's own lines and other agents'
/// events, goes as it is.
#[derive(Default)]
pub struct Encoder {
    n: Option<Box<dyn Neutral>>,
    buf: Vec<u8>,
}

impl Encoder {
    /// A line of the agent's as rush's own events, and None for the host's
    /// own lines. The agent doesn't always start with its type (a turn's
    /// result may not), so a line that doesn't is taken apart to tell.
    fn native_events(&mut self, l: &[u8]) -> Option<Vec<rush_core::event::Event>> {
        let ev = if is_claude_line(l) {
            native_line(l).ok()?
        } else {
            match decode(l).ok()? {
                Line::Native(ev) => ev,
                _ => return None,
            }
        };
        neutral(&mut self.n)?.event(&*ev)
    }

    /// Writes line to w as this client reads it, each line ended with a
    /// newline.
    pub fn encode(&mut self, w: &mut dyn Write, line: &[u8]) -> std::io::Result<()> {
        let Some(evs) = self.native_events(line) else {
            return write_line(w, line); // the host's own, or one it can't read
        };
        for ev in &evs {
            if let Ok(b) = event_line(ev) {
                write_line(w, &b)?;
            }
        }
        Ok(())
    }

    /// Writes a ring line, unpacked, as this client reads it.
    pub fn replay(&mut self, w: &mut dyn Write, l: &[u8]) -> std::io::Result<()> {
        let mut b = std::mem::take(&mut self.buf);
        b.clear();
        crate::pack::unpack(l, &mut b)?;
        let r = self.encode(w, &b);
        self.buf = b;
        r
    }
}

pub(crate) fn write_line(w: &mut dyn Write, l: &[u8]) -> std::io::Result<()> {
    w.write_all(l)?;
    w.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // A client that says hello is known by its protocol; one that says
    // nothing has its first op read as one.
    #[test]
    fn hello_reads_the_protocol_or_leaves_the_op() {
        let (a, mut b) = UnixStream::pair().unwrap();
        b.write_all(b"{\"op\":\"hello\",\"proto\":11}\n{\"op\":\"send\"}\n")
            .unwrap();
        let (p, mut r, first) = hello(&a).unwrap();
        assert_eq!(p, 11);
        assert!(first.is_empty());
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        assert_eq!(l, "{\"op\":\"send\"}\n");

        let (a, mut b) = UnixStream::pair().unwrap();
        b.write_all(b"{\"op\":\"send\",\"text\":\"hi\"}\n").unwrap();
        let (p, _, first) = hello(&a).unwrap();
        assert_eq!(
            (p, first.as_slice()),
            (0, b"{\"op\":\"send\",\"text\":\"hi\"}\n".as_slice())
        );
    }

    // With no native agent registered, the host's own lines and other
    // agents' events go as they are.
    #[test]
    fn host_lines_go_as_they_are() {
        let mut e = Encoder::default();
        let mut out = Vec::new();
        let sent = br#"{"agtop_sent":true,"message":{"content":"hi","role":"user"},"type":"user"}"#;
        e.encode(&mut out, sent).unwrap();
        assert_eq!(out, [sent.as_slice(), b"\n"].concat());
    }
}
