//! Notifications: when a session on this machine or a peer comes to need
//! you, finishes, or fails. And the session list as a stream, so the web
//! app needn't poll.
//!
//! After Go's internal/remote/watch.go (attention :41, with 4d29f271's
//! rule that a turn leaving work running in the background isn't finished
//! :46; Watch :57) and stream.go (sessionsStream :17).

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use rush_host::Info;
use serde_json::json;

use crate::client::peer_sessions;
use crate::ctx::Ctx;
use crate::http::{Request, Response};
use crate::push::Notice;
use crate::server::{Server, Session, sse, sse_headers};

/// What about a session is worth a notification.
pub fn attention(i: &Info) -> &'static str {
    if !i.needs.is_empty() || i.decisions.is_some() {
        "needs you"
    } else if !i.error.is_empty() || i.lost {
        "stopped with an error"
    } else if i.state == "idle" && !i.idle_since.is_zero() && i.background.is_empty() {
        // Not while what it started in the background still runs.
        "finished"
    } else {
        ""
    }
}

/// Go's url.PathEscape, for a machine's name in a link.
fn path_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~$&+,;=:@!'()*".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What the watch has seen of each session: machine/id → its attention,
/// and when it went idle.
#[derive(Default)]
pub struct Seen {
    marks: HashMap<String, String>,
    first: bool,
}

impl Seen {
    pub fn new() -> Seen {
        Seen {
            marks: HashMap::new(),
            first: true,
        }
    }

    /// The notices one look at lists (machine → sessions) makes. What it
    /// saw first is taken as already known.
    pub fn look(&mut self, lists: &[(String, Vec<Session>)]) -> Vec<Notice> {
        let mut out = Vec::new();
        for (m, l) in lists {
            for s in l {
                let a = attention(&s.info);
                let key = format!("{m}/{}", s.info.id);
                let mut mark = a.to_string();
                if a == "finished" {
                    mark.push_str(&s.info.idle_since.to_string());
                }
                let was = self.marks.insert(key.clone(), mark.clone());
                if self.first
                    || (was.is_none() && a == "finished")
                    || was.as_deref() == Some(&mark)
                    || a.is_empty()
                {
                    continue;
                }
                let title = if s.info.name.is_empty() {
                    s.info.id.clone()
                } else {
                    s.info.name.clone()
                };
                out.push(Notice {
                    title,
                    body: format!("{m} · {a}"),
                    url: format!("/#/s/{}/{}", path_escape(m), s.info.id),
                    tag: key,
                });
            }
        }
        self.first = false;
        out
    }
}

impl Server {
    /// Notifies, until ctx ends, when a session on this machine or a peer
    /// comes to need you, finishes, or fails. It looks every few seconds.
    pub fn watch(&self, ctx: &Ctx) {
        let Some(push) = &self.push else {
            return;
        };
        let mut seen = Seen::new();
        loop {
            let mut lists = vec![(self.config.name.clone(), self.sessions())];
            for p in &self.config.peers {
                if let Ok(l) = peer_sessions(p) {
                    lists.push((p.name.clone(), l));
                }
            }
            for n in seen.look(&lists) {
                push.send(&n);
            }
            // Polls lists; a host-pushed feed if 4s is too slow.
            if !ctx.sleep(Duration::from_secs(4)) {
                return;
            }
        }
    }

    /// The session list as Server-Sent Events: {"sessions":[…]} at once and
    /// whenever the list changes, and a keepalive comment every 15s. A
    /// browser's stream starts with {"me":ID}, the device it's signed in as.
    pub(crate) fn sessions_stream(&self, r: &Request, cookie: Option<String>) -> Response {
        let me = cookie.and_then(|c| self.devs.web(&c)).map(|d| d.id);
        let ctx = r.ctx.clone();
        let mut resp = Response::stream(
            200,
            Box::new(move |w: &mut dyn Write| {
                if let Some(id) = me {
                    sse(
                        w,
                        Some(&serde_json::to_vec(&json!({ "me": id })).unwrap_or_default()),
                    );
                }
                // Reads the folder each second per stream.
                let mut l = rush_host::Lister::default();
                let mut last: Vec<u8> = Vec::new();
                let mut n = 0u64;
                loop {
                    let out: Vec<Session> = l
                        .list()
                        .into_iter()
                        .map(|info| {
                            let alive = rush_host::alive(info.host_pid);
                            Session { info, alive }
                        })
                        .collect();
                    let ok = match serde_json::to_vec(&json!({ "sessions": out })) {
                        Ok(b) if b != last => {
                            n = 0;
                            let ok = sse(w, Some(&b));
                            last = b;
                            ok
                        }
                        _ if n % 15 == 14 => sse(w, None),
                        _ => true,
                    };
                    if !ok || !ctx.sleep(Duration::from_secs(1)) {
                        return;
                    }
                    n += 1;
                }
            }),
        );
        sse_headers(&mut resp);
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_host::Task;
    use rush_state::Time;

    fn s(id: &str, f: impl FnOnce(&mut Info)) -> Session {
        let mut info = Info {
            id: id.into(),
            name: format!("n{id}"),
            state: "working".into(),
            ..Info::default()
        };
        f(&mut info);
        Session { info, alive: true }
    }

    /// Idle with work still running in the background isn't finished, so
    /// it isn't pushed as finished until that work is done (4d29f271).
    #[test]
    fn attention_waits_for_background_work() {
        let mut i = Info {
            state: "idle".into(),
            idle_since: Time::now(),
            background: vec![Task {
                id: "wf".into(),
                kind: "local_workflow".into(),
                label: "port".into(),
                ..Task::default()
            }],
            ..Info::default()
        };
        assert_eq!(attention(&i), "");
        i.background.clear();
        assert_eq!(attention(&i), "finished");
        i.needs = "permission".into();
        assert_eq!(attention(&i), "needs you");
        i.needs.clear();
        i.lost = true;
        assert_eq!(attention(&i), "stopped with an error");
    }

    #[test]
    fn notices_only_what_changed() {
        let mut seen = Seen::new();
        let at = Time::from_ms(1_000_000);
        let first = vec![(
            "mac".to_string(),
            vec![s("a", |_| {}), s("b", |i| i.needs = "x".into())],
        )];
        assert!(seen.look(&first).is_empty(), "what it saw first is known");
        let next = vec![(
            "mac".to_string(),
            vec![
                s("a", |i| {
                    i.state = "idle".into();
                    i.idle_since = at;
                }),
                s("b", |i| i.needs = "x".into()),
                // New and already finished: not news.
                s("c", |i| {
                    i.state = "idle".into();
                    i.idle_since = at;
                }),
            ],
        )];
        let got = seen.look(&next);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].title, "na");
        assert_eq!(got[0].body, "mac · finished");
        assert_eq!(got[0].url, "/#/s/mac/a");
        assert_eq!(got[0].tag, "mac/a");
        assert!(seen.look(&next).is_empty());
        assert_eq!(path_escape("my mac/x"), "my%20mac%2Fx");
    }
}
