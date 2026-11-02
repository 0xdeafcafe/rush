//! The sessions of agents on this machine that rush didn't start: one
//! running in a terminal, which can only be seen, or a past one, which can
//! be resumed in rush. After Go's internal/remote/others.go (Other :16,
//! othersMost :26, others :30, listOthers :36, pastOther :74, others
//! handler :83).

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rush_core::adapter::Registry;
use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::http::Response;
use crate::server::{Server, write_json};

/// A session rush didn't start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Other {
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub account: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(rename = "updatedAt", skip_serializing_if = "zero_time")]
    pub updated_at: Time,
    /// Running outside rush now.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub live: bool,
}

fn zero_time(t: &Time) -> bool {
    t.is_zero()
}

const OTHERS_MOST: usize = 200;

/// The listing, kept a minute: it reads the head of every transcript.
static OTHERS: Mutex<Option<(Instant, Vec<Other>)>> = Mutex::new(None);

/// Has the next listing read them again.
pub fn forget() {
    *OTHERS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Them, newest first, leaving out any a rush session is.
pub fn list_others(reg: &Registry) -> Vec<Other> {
    let mut g = OTHERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, l)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return l.clone();
    }
    let ours: HashSet<String> = rush_host::list()
        .into_iter()
        .map(|i| i.session_id)
        .collect();
    let mut out: Vec<Other> = Vec::new();
    let mut seen = HashSet::new();
    for a in reg.all().iter() {
        let Some(d) = a.discoverer() else {
            continue;
        };
        for p in a.profiles() {
            for (ss, live) in [(d.live(&p), true), (d.past(&p), false)] {
                for s in ss {
                    if s.id.is_empty()
                        || s.remote
                        || ours.contains(&s.id)
                        || !seen.insert(s.id.clone())
                    {
                        continue;
                    }
                    out.push(Other {
                        agent: a.kind().as_str().to_string(),
                        account: p.name.clone(),
                        session_id: s.id,
                        name: s.name,
                        cwd: s.cwd,
                        updated_at: if s.updated_at == 0 {
                            Time::default()
                        } else {
                            Time::from_ms(s.updated_at)
                        },
                        live,
                    });
                }
            }
        }
    }
    out.sort_by_key(|o| std::cmp::Reverse(o.updated_at));
    out.truncate(OTHERS_MOST);
    *g = Some((Instant::now(), out.clone()));
    out
}

/// The listed past session with this id, if there is one.
pub fn past_other(reg: &Registry, session_id: &str) -> Option<Other> {
    list_others(reg)
        .into_iter()
        .find(|o| o.session_id == session_id && !o.live)
}

impl Server {
    pub(crate) fn others(&self) -> Response {
        let l = list_others(&self.reg);
        // Go's nil list: null.
        write_json(&(!l.is_empty()).then_some(l))
    }
}
