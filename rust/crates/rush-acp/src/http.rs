//! The one HTTP request each limits reader makes (Kimi's /usages, Code
//! Assist's quota, Z.ai's quota): Go's http.DefaultClient under the
//! caller's context, here bounded by a time of its own.

use std::time::Duration;

/// How long a limits request may take.
const WITHIN: Duration = Duration::from_secs(30);

/// The most of an answer that's read (Go's io.LimitReader of 1 MiB).
const MAX_BODY: u64 = 1 << 20;

/// An answer: its status as a number and as Go says it ("404 Not Found"),
/// and its body.
pub(crate) struct Got {
    pub code: u16,
    pub status: String,
    pub body: Vec<u8>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(WITHIN))
        .http_status_as_error(false)
        .build()
        .into()
}

fn got(res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Got, String> {
    let mut res = res.map_err(|e| e.to_string())?;
    let code = res.status().as_u16();
    let status = res.status().to_string();
    let body = res
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .unwrap_or_default();
    Ok(Got { code, status, body })
}

/// GETs url with a bearer token, asking for JSON.
pub(crate) fn get(url: &str, token: &str) -> Result<Got, String> {
    got(agent()
        .get(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call())
}

/// POSTs body as JSON to url with a bearer token.
pub(crate) fn post(url: &str, token: &str, body: &[u8]) -> Result<Got, String> {
    got(agent()
        .post(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(body))
}

/// A tiny HTTP server for tests: answers each request with what answer
/// gives for its method, path and headers, until dropped.
#[cfg(test)]
pub(crate) mod fake {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// What a request was.
    pub struct Req {
        pub method: String,
        pub path: String,
        pub auth: String,
    }

    /// Answers requests on a port of its own; url is where.
    pub struct Server {
        pub url: String,
    }

    pub fn serve(answer: impl Fn(&Req) -> (u16, String) + Send + Sync + 'static) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut c) = conn else { break };
                let answer = answer.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().unwrap());
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut it = line.split_whitespace();
                    let method = it.next().unwrap_or_default().to_string();
                    let path = it.next().unwrap_or_default().to_string();
                    let (mut auth, mut len) = (String::new(), 0usize);
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                            break;
                        }
                        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
                        match k.trim().to_ascii_lowercase().as_str() {
                            "authorization" => auth = v.trim().to_string(),
                            "content-length" => len = v.trim().parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    let (code, text) = answer(&Req { method, path, auth });
                    let _ = write!(
                        c,
                        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                });
            }
        });
        Server { url }
    }
}
