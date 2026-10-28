//! The little of Go's net/url both modules need from ANTHROPIC_BASE_URL:
//! its scheme, host (with any port), hostname and port.

pub(crate) struct Url<'a> {
    /// Lower-cased, as Go's url.Parse leaves it; empty for `//host`.
    pub scheme: String,
    /// host[:port], as written.
    pub host: &'a str,
}

/// Parses an absolute URL far enough to name its host; None when there is
/// no host to name.
pub(crate) fn parse(s: &str) -> Option<Url<'_>> {
    let s = s.trim();
    let (scheme, rest) = match scheme_end(s) {
        Some(i) => (s[..i].to_ascii_lowercase(), &s[i + 1..]),
        None => (String::new(), s),
    };
    let rest = rest.strip_prefix("//")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    Some(Url { scheme, host })
}

/// Where a scheme ends (its ':'), as Go's getScheme finds it.
fn scheme_end(s: &str) -> Option<usize> {
    for (i, c) in s.char_indices() {
        match c {
            'a'..='z' | 'A'..='Z' => {}
            '0'..='9' | '+' | '-' | '.' if i > 0 => {}
            ':' if i > 0 => return Some(i),
            _ => return None,
        }
    }
    None
}

impl Url<'_> {
    /// The host without its port or IPv6 brackets.
    pub fn hostname(&self) -> &str {
        split_host_port(self.host).0
    }

    /// The port, or "" when none is written.
    pub fn port(&self) -> &str {
        split_host_port(self.host).1
    }
}

fn split_host_port(h: &str) -> (&str, &str) {
    let (mut host, mut port) = (h, "");
    if let Some(i) = h.rfind(':') {
        let p = &h[i + 1..];
        // Go's validOptionalPort: digits after the last ':', and outside
        // any brackets.
        if p.bytes().all(|b| b.is_ascii_digit()) && (!h.starts_with('[') || h[..i].ends_with(']')) {
            host = &h[..i];
            port = p;
        }
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
        host = inner;
    }
    (host, port)
}

/// host:port, with brackets round an IPv6 address, as Go's JoinHostPort.
pub(crate) fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        for (s, scheme, host, hostname, port) in [
            (
                "https://api.anthropic.com",
                "https",
                "api.anthropic.com",
                "api.anthropic.com",
                "",
            ),
            (
                "http://localhost:11434/v1",
                "http",
                "localhost:11434",
                "localhost",
                "11434",
            ),
            (
                "HTTPS://u:p@x.io:8443/a?b#c",
                "https",
                "x.io:8443",
                "x.io",
                "8443",
            ),
            ("http://[::1]:8080/", "http", "[::1]:8080", "::1", "8080"),
            ("http://[::1]/", "http", "[::1]", "::1", ""),
        ] {
            let u = parse(s).unwrap();
            assert_eq!(
                (u.scheme.as_str(), u.host, u.hostname(), u.port()),
                (scheme, host, hostname, port),
                "{s}"
            );
        }
        for s in ["", "localhost:11434", "api.example/v1"] {
            assert!(parse(s).is_none_or(|u| u.hostname().is_empty()), "{s}");
        }
    }
}
