//! One plugin's way out: an HTTP CONNECT proxy on a localhost port of its
//! own, the only address its sandbox may connect to. It tunnels to the
//! hosts the plugin was approved for and nowhere else, and never to a
//! loopback, private or link-local address, whatever a name resolves to.
//! Ported from internal/plugin/proxy.go.
//!
//! One thread waits on the listener (and a waker, so close ends it); each
//! connection has a thread, and a tunnel one more, each blocked on its
//! socket.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::manifest::split_host_port;
use crate::sys::{Ready, Waker, wait_readable};

/// Where a proxy says what it refused and why: Go's *log.Logger.
pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// The most of a CONNECT request read before giving up on it.
const MAX_REQUEST: usize = 64 << 10;

struct Inner {
    allow: HashSet<String>, // host:port
    log: Logger,
    conns: Mutex<HashMap<u64, TcpStream>>,
    next: AtomicU64,
    waker: Arc<Waker>,
}

/// A running proxy for one plugin.
pub struct Proxy {
    port: u16,
    inner: Arc<Inner>,
}

impl Proxy {
    /// Starts a proxy for the given host:port entries.
    pub fn listen(network: &[String], log: Option<Logger>) -> Result<Proxy, String> {
        let mut allow = HashSet::new();
        for n in network {
            let (h, p) = split_host_port(n)?;
            allow.insert(join_host_port(&h, p));
        }
        let ln =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("listen tcp 127.0.0.1:0: {e}"))?;
        ln.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = ln.local_addr().map_err(|e| e.to_string())?.port();
        let inner = Arc::new(Inner {
            allow,
            log: log.unwrap_or_else(|| Arc::new(|_| {})),
            conns: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
            waker: Waker::new().map_err(|e| e.to_string())?,
        });
        let i = inner.clone();
        std::thread::Builder::new()
            .name("plugin-proxy".into())
            .spawn(move || serve(ln, i))
            .map_err(|e| e.to_string())?;
        Ok(Proxy { port, inner })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stops it and cuts every tunnel.
    pub fn close(&self) {
        self.inner.waker.wake();
        for c in self
            .inner
            .conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            let _ = c.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.close();
    }
}

fn join_host_port(h: &str, p: u16) -> String {
    if h.contains(':') {
        format!("[{h}]:{p}")
    } else {
        format!("{h}:{p}")
    }
}

fn serve(ln: TcpListener, inner: Arc<Inner>) {
    loop {
        match wait_readable(ln.as_raw_fd(), &inner.waker, None) {
            Ok(Ready::Fd) => {}
            _ => return,
        }
        match ln.accept() {
            Ok((c, _)) => {
                let _ = c.set_nonblocking(false);
                let i = inner.clone();
                let _ = std::thread::Builder::new()
                    .name("plugin-proxy-conn".into())
                    .spawn(move || handle(c, &i));
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Keeps c to be cut when the proxy closes, until the guard goes.
struct Tracked<'a> {
    inner: &'a Inner,
    id: u64,
}

fn track<'a>(inner: &'a Inner, c: &TcpStream) -> Option<Tracked<'a>> {
    let id = inner.next.fetch_add(1, Ordering::Relaxed);
    inner
        .conns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, c.try_clone().ok()?);
    if inner.waker.woken() {
        // Closed meanwhile: cut it now.
        let _ = c.shutdown(Shutdown::Both);
    }
    Some(Tracked { inner, id })
}

impl Drop for Tracked<'_> {
    fn drop(&mut self) {
        self.inner
            .conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

const FORBIDDEN: &str = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

fn handle(c: TcpStream, inner: &Inner) {
    let Some(_t) = track(inner, &c) else { return };
    let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(rc) = c.try_clone() else { return };
    let mut br = BufReader::new(rc);
    let Some((method, target)) = read_request(&mut br) else {
        return;
    };
    let mut w = &c;
    if method != "CONNECT" {
        let _ = w.write_all(
            b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let ok = split_host_port(&target)
        .ok()
        .filter(|(h, p)| inner.allow.contains(&join_host_port(h, *p)));
    let Some((h, port)) = ok else {
        (inner.log)(&format!("proxy: refused {target}: not approved"));
        let _ = w.write_all(FORBIDDEN.as_bytes());
        return;
    };
    let up = match dial_public(&h, port) {
        Ok(up) => up,
        Err(e) => {
            (inner.log)(&format!("proxy: {target}: {e}"));
            let _ = w.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
    };
    let Some(_tu) = track(inner, &up) else { return };
    let _ = c.set_read_timeout(None);
    if w.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .is_err()
    {
        return;
    }
    let Ok(up_w) = up.try_clone() else { return };
    let Ok(c_w) = c.try_clone() else { return };
    let to_up = std::thread::spawn(move || {
        // Anything the client sent after its request is already buffered.
        let mut up_w = up_w;
        let _ = io::copy(&mut br, &mut up_w);
        let _ = up_w.shutdown(Shutdown::Write);
    });
    let mut up_r = up;
    let mut c_w = c_w;
    let _ = io::copy(&mut up_r, &mut c_w);
    let _ = c_w.shutdown(Shutdown::Write);
    let _ = to_up.join();
}

/// Reads an HTTP request's head, as Go's http.ReadRequest: its method,
/// and its target (the request line's authority for CONNECT, else the
/// Host header). None when it isn't one.
fn read_request(br: &mut BufReader<TcpStream>) -> Option<(String, String)> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = br
            .by_ref()
            .take((MAX_REQUEST - head.len()) as u64)
            .read_until(b'\n', &mut line)
            .ok()?;
        if n == 0 || !line.ends_with(b"\n") {
            return None;
        }
        let blank = line == b"\r\n" || line == b"\n";
        head.extend_from_slice(&line);
        if blank {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
    let first = lines.next()?;
    let mut parts = first.splitn(3, ' ');
    let (method, uri, proto) = (parts.next()?, parts.next()?, parts.next()?);
    if !proto.starts_with("HTTP/") || method.is_empty() {
        return None;
    }
    let host_header = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    let target = if method == "CONNECT" && !uri.starts_with('/') {
        uri.to_string()
    } else {
        host_of(uri).unwrap_or(host_header)
    };
    Some((method.to_string(), target))
}

/// The host of an absolute URI, "http://host/path".
fn host_of(uri: &str) -> Option<String> {
    let (_, rest) = uri.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Connects to host:port, refusing any address that isn't on the public
/// internet, so an approved name can't be pointed at your machine or your
/// network.
fn dial_public(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("lookup {host}: {e}"))?
        .collect();
    let mut last = format!("{host} has no public address");
    for a in addrs {
        if !public(a.ip()) {
            continue;
        }
        match TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            Ok(c) => return Ok(c),
            Err(e) => last = format!("dial tcp {a}: {e}"),
        }
    }
    Err(last)
}

/// Whether ip is on the public internet: Go's IsGlobalUnicast, and none of
/// private, loopback, link-local or carrier-grade NAT (which is private in
/// practice, and where Tailscale puts your machines).
pub fn public(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let global_unicast = !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_link_local()
        && !ip.is_broadcast();
    let private = o[0] == 10 || (o[0] == 172 && o[1] & 0xf0 == 16) || (o[0] == 192 && o[1] == 168);
    let cgnat = o[0] == 100 && o[1] & 0xc0 == 64;
    global_unicast && !private && !cgnat
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let link_local = s[0] & 0xffc0 == 0xfe80;
    let global_unicast =
        !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() && !link_local;
    let private = s[0] & 0xfe00 == 0xfc00;
    global_unicast && !private
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(px: &Proxy, req: &str) -> u16 {
        let mut c = TcpStream::connect(("127.0.0.1", px.port())).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut line = String::new();
        BufReader::new(c).read_line(&mut line).unwrap();
        line.split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    #[test]
    fn tunnels_only_to_approved_public_hosts() {
        // A server on this machine, named by an approved host: the proxy
        // must still refuse it, as a name can be made to point anywhere.
        let ln = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = format!("localhost:{}", ln.local_addr().unwrap().port());
        let px = Proxy::listen(&["api.example.com:443".into(), local.clone()], None).unwrap();
        let cases = [
            (
                "unapproved host",
                "CONNECT evil.example.com:443 HTTP/1.1\r\nHost: evil.example.com:443\r\n\r\n"
                    .to_string(),
                403,
            ),
            (
                "approved host, other port",
                "CONNECT api.example.com:22 HTTP/1.1\r\nHost: api.example.com:22\r\n\r\n".into(),
                403,
            ),
            (
                "ip literal",
                "CONNECT 1.1.1.1:443 HTTP/1.1\r\nHost: 1.1.1.1:443\r\n\r\n".into(),
                403,
            ),
            (
                "approved name for this machine",
                format!("CONNECT {local} HTTP/1.1\r\nHost: {local}\r\n\r\n"),
                502,
            ),
            (
                "plain http",
                "GET http://api.example.com/ HTTP/1.1\r\nHost: api.example.com\r\n\r\n".into(),
                405,
            ),
        ];
        for (name, req, want) in cases {
            assert_eq!(connect(&px, &req), want, "{name}");
        }
        px.close();
        // Closed, it takes no more.
        let after = TcpStream::connect(("127.0.0.1", px.port())).and_then(|mut c| {
            c.set_read_timeout(Some(Duration::from_secs(5)))?;
            c.write_all(b"CONNECT a:1 HTTP/1.1\r\n\r\n")?;
            let mut b = [0u8; 1];
            c.read(&mut b)
        });
        assert!(
            !matches!(after, Ok(n) if n > 0),
            "a closed proxy answered: {after:?}"
        );
    }

    #[test]
    fn public_addresses() {
        for (ip, want) in [
            ("1.1.1.1", true),
            ("2606:4700::1111", true),
            ("127.0.0.1", false),
            ("10.0.0.1", false),
            ("192.168.1.1", false),
            ("172.16.0.1", false),
            ("169.254.169.254", false),
            ("100.100.1.1", false),
            ("::1", false),
            ("fe80::1", false),
            ("fd00::1", false),
            ("0.0.0.0", false),
            ("::ffff:10.0.0.1", false),
        ] {
            assert_eq!(public(ip.parse().unwrap()), want, "public({ip})");
        }
    }
}
