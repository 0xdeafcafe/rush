//! The phones, browsers and SSH keys paired with this machine, each with a
//! credential of its own, so each can be seen and revoked on its own; and
//! what each has open now. After Go's internal/remote/devices.go (Device
//! :22, seenEvery :40, devices :43, hashToken :52, load :58, save :71,
//! add :87, addWeb :98, find :105, touch :125, web :138, ssh :150, all
//! :154, revoke :163, deviceName :177, listDevices :200, revokeDevice :209,
//! liveConns :221, LocalDevices :264, LocalRevoke :271).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rush_state::Time;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{dir, hex, rand_bytes, write_private};
use crate::ctx::Ctx;

/// One paired device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: String,
    /// "web" or "ssh".
    pub kind: String,
    pub name: String,
    pub created: Time,
    #[serde(rename = "lastSeen", skip_serializing_if = "zero_time")]
    pub last_seen: Time,
    /// A web device's cookie, hashed: the cookie is the token.
    #[serde(rename = "tokenHash", skip_serializing_if = "String::is_empty")]
    pub token_hash: String,
    /// An SSH device's public key, as authorized_keys writes it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
    /// How many SSH sessions it has open now; serve fills it in.
    #[serde(skip_serializing_if = "is_zero")]
    pub live: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn zero_time(t: &Time) -> bool {
    t.is_zero()
}

/// How stale last_seen may be on disk: a device in use isn't written each
/// request.
const SEEN_EVERY_NS: i128 = 60_000_000_000;

fn devices_path() -> PathBuf {
    dir().join("remote-devices.json")
}

/// A token's hash, as kept.
pub fn hash_token(t: &str) -> String {
    hex(&Sha256::digest(t.as_bytes()))
}

/// Constant-time equality of a and b.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// remote-devices.json, loaded on first use.
#[derive(Default)]
pub struct Devices {
    inner: Mutex<(bool, Vec<Device>)>,
}

impl Devices {
    fn with<T>(&self, f: impl FnOnce(&mut Vec<Device>) -> T) -> T {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !g.0 {
            g.0 = true;
            if let Ok(b) = std::fs::read(devices_path()) {
                g.1 = serde_json::from_slice::<Option<Vec<Device>>>(&b)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
            }
        }
        f(&mut g.1)
    }

    fn save(list: &[Device]) -> Result<(), String> {
        let b = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        write_private(&devices_path(), &b)
    }

    /// Adds dev with an id and times of its own.
    pub fn add(&self, mut dev: Device) -> Result<Device, String> {
        self.with(|list| {
            dev.id = hex(&rand_bytes(4));
            dev.created = Time::now();
            dev.last_seen = dev.created;
            list.push(dev.clone());
            Devices::save(list).map(|()| dev)
        })
    }

    /// Makes a web device; the token its cookie holds.
    pub fn add_web(&self, name: &str) -> Result<(String, Device), String> {
        let tok = hex(&rand_bytes(32));
        let dev = self.add(Device {
            kind: "web".into(),
            name: name.into(),
            token_hash: hash_token(&tok),
            ..Device::default()
        })?;
        Ok((tok, dev))
    }

    /// The device m picks, seen now.
    fn find(&self, m: impl Fn(&Device) -> bool) -> Option<Device> {
        self.with(|list| {
            let now = Time::now();
            let i = list.iter().position(m)?;
            if now.sub_nanos(list[i].last_seen) > SEEN_EVERY_NS {
                list[i].last_seen = now;
                let _ = Devices::save(list);
            }
            Some(list[i].clone())
        })
    }

    /// Marks device id seen now.
    pub fn touch(&self, id: &str) {
        self.with(|list| {
            let now = Time::now();
            for d in list.iter_mut().filter(|d| d.id == id) {
                d.last_seen = now;
            }
            let _ = Devices::save(list);
        });
    }

    /// The web device whose cookie tok is.
    pub fn web(&self, tok: &str) -> Option<Device> {
        if tok.is_empty() {
            return None;
        }
        let h = hash_token(tok);
        self.find(|d| d.kind == "web" && same(d.token_hash.as_bytes(), h.as_bytes()))
    }

    /// The SSH device with this key (authorized_keys form).
    pub fn ssh(&self, key: &str) -> Option<Device> {
        self.find(|d| d.kind == "ssh" && d.key == key)
    }

    /// Every device, last seen first.
    pub fn all(&self) -> Vec<Device> {
        let mut out = self.with(|l| l.clone());
        out.sort_by_key(|d| std::cmp::Reverse(d.last_seen));
        out
    }

    /// Unpairs device id; false when there's none (or it can't be saved).
    pub fn revoke(&self, id: &str) -> bool {
        self.with(|list| {
            let n = list.len();
            list.retain(|d| d.id != id);
            list.len() != n && Devices::save(list).is_ok()
        })
    }
}

/// A browser's device as people say it, from its User-Agent.
pub fn device_name(ua: &str) -> String {
    let pick = |pairs: &[(&str, &'static str)]| {
        pairs
            .iter()
            .find(|(k, _)| ua.contains(k))
            .map_or("", |(_, v)| *v)
    };
    let dev = pick(&[
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Macintosh", "Mac"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]);
    let app = pick(&[
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]);
    match (dev.is_empty(), app.is_empty()) {
        (false, false) => format!("{dev} · {app}"),
        (true, true) => "a browser".into(),
        _ => format!("{dev}{app}"),
    }
}

/// What each device has open now (event streams, SSH sessions), each
/// ended by its ctx: revoking a device ends them at once.
#[derive(Default)]
pub struct LiveConns {
    by: Mutex<HashMap<String, HashMap<u64, Ctx>>>,
    next: AtomicU64,
}

/// Counts one open thing until it's dropped.
pub struct LiveGuard<'a> {
    l: &'a LiveConns,
    id: String,
    k: u64,
}

impl Drop for LiveGuard<'_> {
    fn drop(&mut self) {
        let mut by = self.l.by.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = by.get_mut(&self.id) {
            m.remove(&self.k);
        }
    }
}

impl LiveConns {
    /// Counts one open thing of device id's, ended by ctx's cancel.
    pub fn add(&self, id: &str, ctx: Ctx) -> LiveGuard<'_> {
        let k = self.next.fetch_add(1, Ordering::Relaxed);
        self.by
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_default()
            .insert(k, ctx);
        LiveGuard {
            l: self,
            id: id.to_string(),
            k,
        }
    }

    /// How many device id has open.
    pub fn count(&self, id: &str) -> i64 {
        let by = self.by.lock().unwrap_or_else(|e| e.into_inner());
        by.get(id).map_or(0, |m| m.len() as i64)
    }

    /// Ends everything device id has open.
    pub fn drop_device(&self, id: &str) {
        let open = self.by.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        for c in open.into_iter().flat_map(|m| m.into_values()) {
            c.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names() {
        for (ua, want) in [
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit Safari/604.1",
                "iPhone · Safari",
            ),
            (
                "Mozilla/5.0 (Macintosh) AppleWebKit Chrome/120 Safari/537",
                "Mac · Chrome",
            ),
            ("Mozilla/5.0 (Windows NT 10.0) Edg/120", "Windows · Edge"),
            ("curl/8", "a browser"),
            ("Firefox/1", "Firefox"),
        ] {
            assert_eq!(device_name(ua), want, "{ua}");
        }
    }

    #[test]
    fn live_conns_end_on_drop() {
        let l = LiveConns::default();
        let c = Ctx::new();
        let g = l.add("a", c.clone());
        assert_eq!(l.count("a"), 1);
        l.drop_device("a");
        assert!(c.done());
        assert_eq!(l.count("a"), 0);
        drop(g);
        let g2 = l.add("b", Ctx::new());
        drop(g2);
        assert_eq!(l.count("b"), 0);
    }
}
