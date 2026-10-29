//! The little of HTTP/1.1 serve needs, on std's sockets: a thread per
//! connection, requests read whole (a body by its Content-Length), and a
//! response either whole or streamed until the client goes (Server-Sent
//! Events). Nothing runs while no request comes: the accept blocks.
//! Stands in for Go's net/http server as server.go's Run uses it
//! (ReadHeaderTimeout 10s, Shutdown on the end).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use crate::ctx::Ctx;

/// The most a request's body may be: an op's, its images base64 in it
/// (server.go opMost), and a little.
pub const BODY_MOST: usize = (64 << 20) + 1024;

/// One request.
pub struct Request {
    pub method: String,
    /// The path as sent, before its query.
    pub path: String,
    pub query: String,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Over TLS: never, here; what's in front of serve says so in a header.
    pub tls: bool,
    /// Ends when the server does, or what owns the request ends it.
    pub ctx: Ctx,
    /// The body was longer than BODY_MOST.
    pub too_big: bool,
}

impl Request {
    /// A request for tests and for handing on.
    pub fn new(method: &str, target: &str) -> Request {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        Request {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            headers: Vec::new(),
            body: Vec::new(),
            tls: false,
            ctx: Ctx::new(),
            too_big: false,
        }
    }

    /// With header name set to v.
    pub fn with_header(mut self, name: &str, v: &str) -> Request {
        self.headers
            .push((name.to_ascii_lowercase(), v.to_string()));
        self
    }

    /// With body b.
    pub fn with_body(mut self, b: impl Into<Vec<u8>>) -> Request {
        self.body = b.into();
        self
    }

    /// Header name's first value, "" when it has none.
    pub fn header(&self, name: &str) -> &str {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map_or("", |(_, v)| v.as_str())
    }

    /// The cookie called name.
    pub fn cookie(&self, name: &str) -> Option<String> {
        for (n, v) in &self.headers {
            if n != "cookie" {
                continue;
            }
            for part in v.split(';') {
                if let Some((k, v)) = part.trim().split_once('=')
                    && k.trim() == name
                {
                    let v = v.trim();
                    let v = v
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .unwrap_or(v);
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    /// Query parameter k's value.
    pub fn query_get(&self, k: &str) -> Option<String> {
        self.query.split('&').find_map(|kv| {
            let (a, b) = kv.split_once('=').unwrap_or((kv, ""));
            (unescape(a) == k).then(|| unescape(b))
        })
    }

    /// The Host it was asked for.
    pub fn host(&self) -> &str {
        self.header("host")
    }
}

/// %xx and + in a query part.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Writes a streamed response's body, flushing as it goes.
pub type StreamFn = Box<dyn FnOnce(&mut dyn Write) + Send>;

/// What a response carries.
pub enum Body {
    Bytes(Vec<u8>),
    /// Written until it returns, then the connection closes.
    Stream(StreamFn),
}

/// One response.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Body::Bytes(body.into()),
        }
    }

    /// One streamed by f.
    pub fn stream(status: u16, f: StreamFn) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Body::Stream(f),
        }
    }

    /// Sets header name, replacing it.
    pub fn set(&mut self, name: &str, v: &str) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), v.to_string()));
    }

    /// Adds a value for header name.
    pub fn add(&mut self, name: &str, v: &str) {
        self.headers.push((name.to_string(), v.to_string()));
    }

    /// Header name's first value.
    pub fn get(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }

    /// The body, when it's whole.
    pub fn bytes(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(b) => b,
            Body::Stream(_) => &[],
        }
    }
}

/// HTTP's words for a status (Go's http.StatusText).
pub fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Request Entity Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        _ => "",
    }
}

/// Answers a request.
pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// Serves h on ln until ctx ends: each connection on a thread of its own,
/// its requests one after another. The end stops the accepting at once;
/// a stream running then ends with ctx, as its handler watches it.
pub fn serve(ctx: &Ctx, ln: TcpListener, h: Handler) -> std::io::Result<()> {
    let addr = ln.local_addr()?;
    {
        // The end wakes the accept by connecting to it.
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("serve-stop".into())
            .spawn(move || {
                ctx.wait();
                let _ = TcpStream::connect(addr);
            })?;
    }
    loop {
        let (conn, _) = match ln.accept() {
            Ok(c) => c,
            Err(e) => {
                if ctx.done() {
                    return Ok(());
                }
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
        };
        if ctx.done() {
            return Ok(());
        }
        let (h, ctx) = (h.clone(), ctx.clone());
        let _ = std::thread::Builder::new()
            .name("serve-conn".into())
            .spawn(move || connection(conn, &h, &ctx));
    }
}

/// Serves one connection's requests until it closes or a stream ends it.
fn connection(conn: TcpStream, h: &Handler, ctx: &Ctx) {
    let Ok(w) = conn.try_clone() else {
        return;
    };
    let mut r = BufReader::new(conn);
    let mut w = w;
    loop {
        // ReadHeaderTimeout, and an idle connection is let go after a while.
        let _ = r.get_ref().set_read_timeout(Some(Duration::from_secs(10)));
        let Some(mut req) = read_request(&mut r) else {
            return;
        };
        if ctx.done() {
            return;
        }
        req.ctx = ctx.child();
        let close = req.header("connection").eq_ignore_ascii_case("close");
        let resp = if req.too_big {
            Response::new(413, "request too large\n")
        } else {
            h(&req)
        };
        let keep = write_response(&mut w, &req, resp) && !close;
        req.ctx.cancel();
        if !keep {
            let _ = w.shutdown(Shutdown::Both);
            return;
        }
    }
}

/// Reads one request; None when the connection ends or isn't HTTP.
fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    // Blank lines before a request are allowed.
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut req = Request::new(&method, &target);
    let mut size = line.len();
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        size += line.len();
        if size > 1 << 20 {
            return None;
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            break;
        }
        let (n, v) = l.split_once(':')?;
        req.headers
            .push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let n: usize = req.header("content-length").parse().unwrap_or(0);
    if n > BODY_MOST {
        req.too_big = true;
        return Some(req);
    }
    let _ = r.get_ref().set_read_timeout(Some(Duration::from_secs(60)));
    let mut body = vec![0; n];
    r.read_exact(&mut body).ok()?;
    req.body = body;
    Some(req)
}

/// Writes resp; whether the connection may carry another request.
fn write_response(w: &mut TcpStream, req: &Request, resp: Response) -> bool {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, status_text(resp.status));
    let has = |n: &str| resp.headers.iter().any(|(h, _)| h.eq_ignore_ascii_case(n));
    for (n, v) in &resp.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    if !has("date") {
        head.push_str(&format!("Date: {}\r\n", http_date()));
    }
    match resp.body {
        Body::Bytes(b) => {
            let no_body = resp.status == 204 || resp.status == 304;
            if !no_body {
                if !has("content-type") && !b.is_empty() {
                    head.push_str("Content-Type: text/plain; charset=utf-8\r\n");
                }
                head.push_str(&format!("Content-Length: {}\r\n", b.len()));
            }
            head.push_str("\r\n");
            let mut out = head.into_bytes();
            if req.method != "HEAD" && !no_body {
                out.extend_from_slice(&b);
            }
            w.write_all(&out).is_ok() && w.flush().is_ok()
        }
        Body::Stream(f) => {
            head.push_str("Connection: close\r\n\r\n");
            if w.write_all(head.as_bytes()).is_err() {
                return false;
            }
            let _ = w.flush();
            let mut fw = Flushing(w);
            f(&mut fw);
            false
        }
    }
}

/// A writer that flushes each write: a stream's events go out at once.
struct Flushing<'a>(&'a mut TcpStream);

impl Write for Flushing<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(b)?;
        self.0.flush()?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// Now as HTTP's Date header has it.
fn http_date() -> String {
    let t = rush_state::Time::now();
    let secs = t.unix_nanos().div_euclid(1_000_000_000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    const WD: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WD[days.rem_euclid(7) as usize],
        d,
        MO[(m - 1) as usize],
        y,
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 as a date (Howard Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookies_and_queries() {
        let r = Request::new("GET", "/api/x?full=1&a=b%20c")
            .with_header("Cookie", "a=1; rush_token=\"tok\"; b=2");
        assert_eq!(r.cookie("rush_token").as_deref(), Some("tok"));
        assert_eq!(r.cookie("none"), None);
        assert_eq!(r.query_get("full").as_deref(), Some("1"));
        assert_eq!(r.query_get("a").as_deref(), Some("b c"));
        assert_eq!(r.path, "/api/x");
    }

    #[test]
    fn dates_are_httpish() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert!(http_date().ends_with(" GMT"));
    }

    #[test]
    fn serves_on_loopback_until_the_end() {
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ln.local_addr().unwrap();
        let ctx = Ctx::new();
        let c = ctx.clone();
        let t = std::thread::spawn(move || {
            serve(
                &c,
                ln,
                Arc::new(|r: &Request| {
                    Response::new(200, format!("{} {}", r.method, r.body.len()))
                }),
            )
        });
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(
            b"POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
        assert!(out.ends_with("\r\n\r\nPOST 3"), "{out}");
        ctx.cancel();
        t.join().unwrap().unwrap();
    }
}
