//! Settings › Remote: this machine's sessions from your phone and your
//! other machines. Remote mode keeps rush serve running from login on,
//! reached over your tailnet; Pair a device shows a code that signs a
//! device in once. After Go's internal/ui/settings_remote.go (remotePage
//! :22, remoteSnap :28, remoteNow :40, switchRow :76, serveSwitch :93,
//! remoteSections :117, deviceRow :207, seenAgo :245, pairSheet :262,
//! openPairSheet :269, watch :280, body :302, key :350), and of
//! internal/remote what the page reads: remote.json (config.go:22, :81,
//! :112), the token (:177), serve's devices and pairing (devices.go:255,
//! :262, :268; service.go:27, :33, :36, :107; pair.go:25, :92) and its
//! plugins' names (plugins.go:22). The QR code is qrLines :354, its
//! bottle qrBottle :406 and colours qrInk :420, drawn from qr.rs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rush_core::text::{Color, Line, Mods, Span, Style};
use rush_core::time::Ms;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::form::{Section, Setting};
use super::paint::{civil, trunc};
use super::qr;
use super::{Page, State};
use crate::frame::sheet::{Sheet, sheet_title};
use crate::frame::{Out, Press};
use crate::model::Model;
use crate::style::*;

/// serve's own plugins (remote.Plugin*).
pub const PLUGIN_REMOTE: &str = "remote";
pub const PLUGIN_TAILSCALE: &str = "remote-tailscale";
pub const PLUGIN_CLOUDFLARED: &str = "remote-cloudflared";
pub const PLUGIN_SSH: &str = "remote-ssh";

/// Where serve listens unless told (remote.DefaultListen).
const DEFAULT_LISTEN: &str = "127.0.0.1:7878";

/// How long a reading of the switches stays fresh.
const FRESH: Duration = Duration::from_secs(2);

/// remote.json's parts the page shows (remote.Config).
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub listen: String,
    pub web: bool,
    pub attach_peers: bool,
    pub peers: usize,
    /// "" this machine only, "tailscale", or "cloudflare".
    pub reach: String,
}

/// A device paired with this machine's serve (remote.Device).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: String,
    /// "web" or "ssh".
    pub kind: String,
    pub name: String,
    pub created: String,
    #[serde(rename = "lastSeen")]
    pub last_seen: String,
    pub fingerprint: String,
    /// How many SSH sessions it has open now.
    pub live: i64,
}

/// A code that pairs a device (remote.Pairing).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Pairing {
    pub code: String,
    pub url: String,
    pub expires: String,
    pub ssh: String,
    #[serde(rename = "hostKey")]
    pub host_key: String,
}

/// The switches as last read, off the UI (remoteSnap).
#[derive(Clone, Debug, Default)]
pub struct Snap {
    pub at: Option<Instant>,
    pub reading: bool,
    pub mode: bool,
    pub on: BTreeMap<String, bool>,
    pub cfg: Config,
    pub devs: Vec<Device>,
    /// serve didn't answer.
    pub dev_err: Option<String>,
    /// The device x was pressed on once: a second x unpairs it.
    pub unpairing: String,
}

/// The page.
pub fn page() -> Page {
    Page {
        name: "Remote",
        form: Some(sections),
        prep: Some(want),
        ..Page::default()
    }
}

fn state_dir() -> PathBuf {
    rush_state::dir()
}

/// remote.json (configPath).
fn config_path() -> PathBuf {
    state_dir().join("remote.json")
}

/// The kept serve's log (ServiceLog).
pub fn service_log() -> String {
    state_dir().join("serve.log").to_string_lossy().into_owned()
}

/// Whether remote mode is on: serve is kept running (RemoteOn).
fn remote_on() -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join("Library/LaunchAgents/com.github.0xdeafcafe.rush.serve.plist")
        .exists()
}

/// remote.json as written (loadRaw).
fn load_raw() -> Result<Map<String, Value>, String> {
    match std::fs::read(config_path()) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// remote.json with its defaults, and how serve is reached as its
/// plugins say (LoadConfig, withSwitches).
fn load_config() -> Config {
    let raw = load_raw().unwrap_or_default();
    let b = |k: &str| raw.get(k).and_then(Value::as_bool).unwrap_or(false);
    let listen = raw
        .get("listen")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_LISTEN)
        .to_string();
    let reach = if rush_plugin::bundled_on(PLUGIN_TAILSCALE) {
        "tailscale"
    } else if rush_plugin::bundled_on(PLUGIN_CLOUDFLARED) {
        "cloudflare"
    } else {
        ""
    };
    Config {
        listen,
        web: b("web"),
        attach_peers: b("attachPeers"),
        peers: raw
            .get("peers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        reach: reach.into(),
    }
}

/// Changes one of remote.json's switches, keeping the rest as written
/// (UpdateConfig).
fn update_config(key: &str, on: bool) -> Result<(), String> {
    let mut raw = load_raw()?;
    if on {
        raw.insert(key.into(), Value::Bool(true));
    } else {
        raw.remove(key);
    }
    rush_state::store::write_json(&config_path(), &Value::Object(raw)).map_err(|e| e.to_string())
}

/// serve's token, when it has made one (Token).
fn token() -> Result<String, String> {
    let b = std::fs::read(state_dir().join("remote-token"))
        .map_err(|_| "rush serve isn't answering: turn Remote mode on".to_string())?;
    let t = String::from_utf8_lossy(&b).trim().to_string();
    if t.len() < 32 {
        return Err("rush serve isn't answering: turn Remote mode on".into());
    }
    Ok(t)
}

/// One request to this machine's serve, with its token (localCall).
fn local_call<T: for<'de> Deserialize<'de>>(post: bool, path: &str) -> Result<T, String> {
    let cfg = load_config();
    let tok = token()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("http://{}{path}", cfg.listen);
    let auth = format!("Bearer {tok}");
    let res = if post {
        agent
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .send("{}")
    } else {
        agent.get(&url).header("Authorization", &auth).call()
    };
    let mut res = res.map_err(|_| "rush serve isn't answering: turn Remote mode on".to_string())?;
    let status = res.status();
    let b = res
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if status != 200 {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct E {
            #[serde(alias = "error", alias = "Error")]
            error: String,
        }
        let e: E = serde_json::from_slice(&b).unwrap_or_default();
        return Err(if e.error.is_empty() {
            status.to_string()
        } else {
            e.error
        });
    }
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}

/// The devices paired with this machine's serve (LocalDevices).
fn local_devices() -> Result<Vec<Device>, String> {
    local_call::<Option<Vec<Device>>>(false, "/api/devices").map(Option::unwrap_or_default)
}

/// The code split for typing: "ABCD-EFGH" (PairCode).
pub fn pair_code(c: &str) -> String {
    if c.len() != 8 || !c.is_ascii() {
        return c.to_string();
    }
    format!("{}-{}", &c[..4], &c[4..])
}

/// Reads the switches again, off the UI, when the last reading is more
/// than a moment old (remoteNow).
pub fn want(m: &mut Model) {
    let st = &mut m.kept::<State>().remote;
    if st.reading || st.at.is_some_and(|a| a.elapsed() < FRESH) {
        return;
    }
    st.reading = true;
    m.later(
        || {
            let mode = remote_on();
            let cfg = load_config();
            let on: BTreeMap<String, bool> = [
                PLUGIN_REMOTE,
                PLUGIN_TAILSCALE,
                PLUGIN_CLOUDFLARED,
                PLUGIN_SSH,
            ]
            .into_iter()
            .map(|n| (n.to_string(), rush_plugin::bundled_on(n)))
            .collect();
            let devs = local_devices();
            (mode, cfg, on, devs)
        },
        |m, (mode, cfg, on, devs)| {
            let st = &mut m.kept::<State>().remote;
            st.at = Some(Instant::now());
            st.reading = false;
            st.mode = mode;
            st.cfg = cfg;
            st.on = on;
            match devs {
                Ok(d) => {
                    st.devs = d;
                    st.dev_err = None;
                }
                Err(e) => {
                    st.devs.clear();
                    st.dev_err = Some(e);
                }
            }
            Out::Draw
        },
    );
}

/// Has the next key read the switches again (remoteChanged).
fn changed(m: &mut Model) {
    m.kept::<State>().remote.at = None;
    want(m);
}

/// on or off (onOff).
fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// What a change off the UI says once it's done (doneMsg).
fn done(m: &mut Model, text: String, err: Option<String>) -> Out {
    match err {
        Some(e) => m.flash_err(&e),
        None => m.flash(&text),
    }
    changed(m);
    Out::Draw
}

/// One of remote.json's switches as a setting (switchRow).
fn switch_row(
    label: &str,
    key: &'static str,
    on: bool,
    what: &str,
    if_on: &str,
    if_off: &str,
) -> Setting {
    let l = label.to_lowercase();
    let mut st = Setting {
        label: label.into(),
        value: on_off(on).into(),
        choices: vec!["off".into(), "on".into()],
        unset: "off".into(),
        what: what.into(),
        ..Setting::default()
    };
    st.means.insert("on".into(), if_on.into());
    st.means.insert("off".into(), if_off.into());
    st.run = Some(Rc::new(move |m, v| {
        let (l, v) = (l.clone(), v.to_string());
        m.later(
            {
                let v = v.clone();
                move || update_config(key, v == "on").err()
            },
            move |m, err| done(m, format!("{l} {v}"), err),
        );
        Out::Draw
    }));
    st
}

/// One of serve's plugins as a setting: the same switch as Settings ›
/// Plugins (serveSwitch).
fn serve_switch(
    label: &str,
    name: &'static str,
    on: bool,
    what: &str,
    if_on: &str,
    if_off: &str,
    said: Option<fn(bool) -> String>,
) -> Setting {
    let l = label.to_lowercase();
    let mut st = Setting {
        label: label.into(),
        value: on_off(on).into(),
        choices: vec!["off".into(), "on".into()],
        unset: "off".into(),
        what: format!("{what} (the {name} plugin)"),
        ..Setting::default()
    };
    st.means.insert("on".into(), if_on.into());
    st.means.insert("off".into(), if_off.into());
    st.run = Some(Rc::new(move |m, v| {
        let turn_on = v == "on";
        let text = match said {
            Some(f) => f(turn_on),
            None => format!("{l} {v}"),
        };
        m.later(
            move || {
                rush_plugin::set_bundled(name, turn_on)
                    .and_then(|()| rush_plugin::client::reload())
                    .err()
            },
            move |m, err| done(m, text, err),
        );
        Out::Draw
    }));
    st
}

/// The page's sections (remoteSections).
fn sections(m: &Model) -> Vec<Section> {
    let snap = m
        .kept
        .peek::<State>()
        .map(|s| s.remote.clone())
        .unwrap_or_default();
    let on = |n: &str| snap.on.get(n).copied().unwrap_or(false);
    let mode = serve_switch(
        "Remote mode",
        PLUGIN_REMOTE,
        on(PLUGIN_REMOTE) || snap.mode,
        "Keeps rush serve running from login on, even with rush closed, reached and let in as the rows below say: your sessions on your phone or another device, at home or not",
        "serve runs at login and after a crash, reached as the rows below say.",
        "serve is stopped and its addresses go; your sessions are only on this machine.",
        Some(|on| {
            if on {
                "remote mode on · Pair a device when it's up".into()
            } else {
                "remote mode off".into()
            }
        }),
    );
    let tailnet = serve_switch(
        "Over Tailscale",
        PLUGIN_TAILSCALE,
        on(PLUGIN_TAILSCALE),
        "Devices signed in to your tailnet reach serve, and nothing else does",
        "https on port 8443, and SSH on 2222 with the terminal on.",
        "nothing on your tailnet reaches serve.",
        None,
    );
    let tunnel = serve_switch(
        "Through Cloudflare",
        PLUGIN_CLOUDFLARED,
        on(PLUGIN_CLOUDFLARED),
        "Serve's web app is reached through your own Cloudflare tunnel, its token in cloudflared-token in rush's folder",
        "the web app is at your tunnel's address.",
        "the tunnel is stopped.",
        None,
    );
    let web = switch_row(
        "Web app",
        "web",
        snap.cfg.web,
        "The app your phone and browsers open, signed in by pairing",
        "it's served; a device signs in with a pairing code.",
        "only rush on your other machines can use serve.",
    );
    let ssh = serve_switch(
        "Terminal (SSH)",
        PLUGIN_SSH,
        on(PLUGIN_SSH),
        "Rush itself, the whole terminal app, from any SSH app (Termius, Blink, ssh) with a paired key. Like tmux, with no tmux: each device has its own rush, kept 30 minutes after it drops",
        "reconnect and it's as you left it.",
        "serve's SSH stops, and with it each device's rush.",
        None,
    );
    let pair = Setting {
        label: "Pair a device".into(),
        value: "a QR code for a phone, a code for SSH".into(),
        about: Some(Rc::new(|| {
            (
                "Pair a device".into(),
                "Shows a QR code for your phone's camera: it opens the app and signs it in. With SSH on, the same code pairs an SSH key: connect, and type it when asked. A code works once, within 10 minutes. The device needs Tailscale signed in to your tailnet. On an iPhone, Share › Add to Home Screen afterwards, for notifications too.".into(),
                String::new(),
            )
        })),
        key: Some(Rc::new(|m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            open_pair_sheet(m);
            Out::Draw
        })),
        keys: vec!["enter".into(), "pair a device".into()],
        ..Setting::default()
    };
    // As Go paints it: running in green, the rest dim; not answering in
    // orange.
    let styled = match &snap.dev_err {
        Some(_) if snap.mode => line([
            sp("starting, or not answering", ORANGE),
            sp(format!(" · {}", service_log()), DIM),
        ]),
        Some(_) => line([sp("not running", DIM)]),
        None => {
            let live: i64 = snap.devs.iter().map(|d| d.live.min(1)).sum();
            line([
                sp("running", GREEN),
                sp(
                    format!(" · {} paired, {live} connected now", snap.devs.len()),
                    DIM,
                ),
            ])
        }
    };
    let status = Setting {
        label: "Serve".into(),
        value: styled.plain(),
        styled,
        about: Some(Rc::new(|| {
            (
                "Serve".into(),
                format!(
                    "Whether rush serve is up on this machine, and how many devices are paired with it and connected now. Its log is {}.",
                    service_log()
                ),
                String::new(),
            )
        })),
        ..Setting::default()
    };
    let mut out = vec![Section {
        title: "Remote".into(),
        note: "your sessions from your phone and other devices".into(),
        rows: vec![mode, status, tailnet, tunnel, web, ssh, pair],
        ..Section::default()
    }];
    if !snap.devs.is_empty() {
        out.push(Section {
            title: "Devices".into(),
            note: "paired with this machine · x unpairs one".into(),
            rows: snap
                .devs
                .iter()
                .map(|d| device_row(m, d.clone(), &snap.unpairing))
                .collect(),
            ..Section::default()
        });
    }
    if snap.cfg.peers > 0 {
        out.push(Section {
            title: "Other machines".into(),
            note: "added with rush remote add".into(),
            rows: vec![switch_row(
                "Their sessions here",
                "attachPeers",
                snap.cfg.attach_peers,
                "Lists the sessions of the machines you've added in this rush, as rush remote attach does, kept by serve with no terminal of its own",
                "they list here as name @machine.",
                "only this machine's sessions list here.",
            )],
            ..Section::default()
        });
    }
    out
}

/// An RFC 3339 time in Unix ms; 0 when it isn't one.
fn parse_time(s: &str) -> Ms {
    rush_sys::timefmt::parse(s)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as Ms)
}

/// How long ago t was, as people say it (seenAgo).
pub fn seen_ago(t: Ms, now: Ms) -> String {
    if t <= 0 {
        return "never".into();
    }
    let d = now - t;
    const MIN: Ms = 60_000;
    if d < MIN {
        "just now".into()
    } else if d < 60 * MIN {
        format!("{}m ago", d / MIN)
    } else if d < 48 * 60 * MIN {
        format!("{}h ago", d / (60 * MIN))
    } else {
        let c = civil(t);
        format!("{} {}", c.day, c.month_name())
    }
}

/// A paired device: what it is, when it was last seen, and x twice to
/// unpair it (deviceRow).
fn device_row(m: &Model, d: Device, unpairing: &str) -> Setting {
    let icon = match d.kind.as_str() {
        "web" => "◰",
        "ssh" => "❯",
        _ => "",
    };
    let seen = if unpairing == d.id {
        line([sp("x again to unpair", RED)])
    } else if d.live == 0 {
        line([sp(
            format!("seen {}", seen_ago(parse_time(&d.last_seen), m.now)),
            DIM,
        )])
    } else {
        line([sp("● connected", GREEN)])
    };
    let (d1, d2) = (d.clone(), d.clone());
    Setting {
        label: format!("{icon} {}", d.name),
        value: seen.plain(),
        styled: seen,
        about: Some(Rc::new(move || {
            let d = &d1;
            let what = match d.kind.as_str() {
                "web" => {
                    "A browser signed in with a pairing code or the token: its cookie is its own."
                        .to_string()
                }
                "ssh" => format!(
                    "An SSH key, paired with a code: it gets in to rush with no password. {}",
                    d.fingerprint
                ),
                _ => String::new(),
            };
            let c = civil(parse_time(&d.created));
            (
                d.name.clone(),
                format!(
                    "{what} Paired {} {} {:02}:{:02}.",
                    c.day,
                    c.month_name(),
                    c.hour,
                    c.minute
                ),
                String::new(),
            )
        })),
        key: Some(Rc::new(move |m, s| {
            let st = &mut m.kept::<State>().remote;
            if s != "x" {
                st.unpairing.clear();
                return Out::Pass;
            }
            if st.unpairing != d2.id {
                st.unpairing = d2.id.clone();
                return Out::Draw;
            }
            st.unpairing.clear();
            let (id, name) = (d2.id.clone(), d2.name.clone());
            m.later(
                move || local_call::<Value>(true, &format!("/api/devices/{id}/revoke")).err(),
                move |m, err| done(m, format!("unpaired {name}"), err),
            );
            Out::Draw
        })),
        keys: vec!["x".into(), "unpair".into()],
        ..Setting::default()
    }
}

/// The header's bottle (clanker.go) a pixel to a module: its cap, neck
/// and shoulders, the yellow label with a red band for RUSH, and its body
/// (qrBottle).
const QR_BOTTLE: [&str; 10] = [
    "GKGKGK", "KGKGKG", "kkkkkk", ".dAAa.", "dAAAad", "YYYYYY", "YRRRRY", "YYYYYY", "dAAAad",
    "dDDDDd",
];

/// The bottle's colours, as clkInk's (qrInk).
fn qr_ink(c: u8) -> Option<Color> {
    Some(match c {
        b'K' => Color::Rgb(64, 64, 68),
        b'G' => Color::Rgb(112, 112, 116),
        b'k' => Color::Rgb(88, 88, 92),
        b'A' => Color::Rgb(154, 76, 20),
        b'a' => Color::Rgb(212, 128, 44),
        b'd' => Color::Rgb(104, 46, 8),
        b'D' => Color::Rgb(84, 38, 6),
        b'Y' => Color::Rgb(250, 214, 30),
        b'R' => Color::Rgb(214, 28, 24),
        _ => return None,
    })
}

/// The QR code's colours: fixed, not moved onto the terminal's ground, so
/// it scans the same on a light one.
const QR_LIGHT: Color = Color::Rgb(240, 236, 228);
const QR_DARK: Color = Color::Rgb(30, 28, 26);
const QR_EYE: Color = Color::Rgb(190, 92, 60);

/// text as a QR code, two modules to a line with half blocks, in rush's
/// colours whatever the terminal's: its dark ground on its cream, the
/// three finder eyes in its orange (dark enough to read as dark), a quiet
/// zone of cream, and the rush bottle in the middle, one module to its
/// pixel, which the code's error correction reads past (qrLines).
pub fn qr_lines(text: &str) -> Vec<Line> {
    let mut c = qr::encode(text, qr::Level::H);
    if c.as_ref().is_ok_and(|c| c.size >= 45) {
        // From version 7 an alignment pattern sits in the middle, under
        // the bottle: a little less correction keeps the code smaller and
        // clear.
        c = qr::encode(text, qr::Level::Q);
    }
    let c = match c {
        Ok(c) => c,
        Err(e) => return vec![line([sp(e, RED)])],
    };
    const QUIET: isize = 2;
    let size = c.size as isize;
    let n = size + 2 * QUIET;
    let eye = |x: isize, y: isize| {
        let inn = |v: isize, at: isize| v >= at + 2 && v <= at + 4;
        let far = size - 7;
        inn(x, 0) && inn(y, 0) || inn(x, far) && inn(y, 0) || inn(x, 0) && inn(y, far)
    };
    let (bw, bh) = (
        QR_BOTTLE[0].len() as isize + 2,
        QR_BOTTLE.len() as isize + 2,
    );
    let (bx, by) = ((size - bw) / 2, (size - bh) / 2);
    let rgb = |x: isize, y: isize| -> Color {
        let (x, y) = (x - QUIET, y - QUIET);
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            let (px, py) = (x - bx - 1, y - by - 1);
            if py >= 0
                && (py as usize) < QR_BOTTLE.len()
                && px >= 0
                && (px as usize) < QR_BOTTLE[py as usize].len()
                && let Some(ink) = qr_ink(QR_BOTTLE[py as usize].as_bytes()[px as usize])
            {
                return ink;
            }
            return QR_LIGHT;
        }
        if !c.black(x, y) {
            QR_LIGHT
        } else if eye(x, y) {
            QR_EYE
        } else {
            QR_DARK
        }
    };
    (0..n)
        .step_by(2)
        .map(|y| {
            let mut l = Line::new();
            for x in 0..n {
                l.spans.push(Span::new(
                    "▀",
                    Style {
                        fg: rgb(x, y),
                        bg: rgb(x, y + 1),
                        mods: Mods::NONE,
                    },
                ));
            }
            l
        })
        .collect()
}

/// Shows a code that pairs a device with this machine's serve, and
/// watches for the device to turn up (pairSheet).
pub struct PairSheet {
    p: Pairing,
    err: Option<String>,
    opened: Ms,
    paired: Option<Device>,
    /// Set once the sheet is gone: its watch stops.
    gone: Arc<AtomicBool>,
}

impl Drop for PairSheet {
    fn drop(&mut self) {
        self.gone.store(true, Ordering::Relaxed);
    }
}

/// Opens the sheet and asks serve for a code (openPairSheet).
pub fn open_pair_sheet(m: &mut Model) {
    let gone = Arc::new(AtomicBool::new(false));
    m.open_sheet(Box::new(PairSheet {
        p: Pairing::default(),
        err: None,
        opened: m.now,
        paired: None,
        gone: gone.clone(),
    }));
    m.later(
        || local_call::<Pairing>(true, "/api/pair"),
        |m, r| {
            if let Some(s) = m.sheet_as::<PairSheet>() {
                match r {
                    Ok(p) => s.p = p,
                    Err(e) => s.err = Some(e),
                }
            }
            Out::Draw
        },
    );
    watch(m, gone);
}

/// Looks for a device paired since the sheet opened, every 2s while it's
/// open (watch). Only with a loop to send to.
fn watch(m: &Model, gone: Arc<AtomicBool>) {
    let Some(tx) = m.sender() else {
        return;
    };
    let tx = tx.lane(1);
    let _ = std::thread::Builder::new()
        .name("rush-pair-watch".into())
        .spawn(move || {
            while !gone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(2));
                if gone.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(devs) = local_devices() else {
                    continue;
                };
                let sent = tx.send(crate::worker::Note::Apply(Box::new(
                    move |m: &mut Model| {
                        let Some(s) = m.sheet_as::<PairSheet>() else {
                            return Out::Idle;
                        };
                        if s.paired.is_some() {
                            return Out::Idle;
                        }
                        if let Some(d) =
                            devs.into_iter().find(|d| parse_time(&d.created) > s.opened)
                        {
                            s.paired = Some(d);
                            s.gone.store(true, Ordering::Relaxed);
                            changed(m);
                            return Out::Draw;
                        }
                        Out::Idle
                    },
                )));
                if !sent {
                    return;
                }
            }
        });
}

/// l in the middle of w cells.
fn center_line(l: Line, w: usize) -> Line {
    let pad = w.saturating_sub(l.width());
    let mut out = line([plain(blanks(pad / 2))]);
    join(&mut out, l);
    out
}

impl Sheet for PairSheet {
    fn width(&self, _m: &Model) -> usize {
        72
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut out = vec![
            sheet_title("Pair a device", "a phone's camera, or an SSH key", w),
            Line::new(),
        ];
        if let Some(d) = &self.paired {
            out.extend([
                Line::new(),
                center_line(line([sp("✓ Paired", GREEN)]), w),
                Line::new(),
                center_line(line([plain(d.name.clone())]), w),
                Line::new(),
                center_line(
                    line([sp("It's under Devices in Settings › Remote.", DIM)]),
                    w,
                ),
                Line::new(),
                keys_fit(w, &[("enter", "pair another"), ("esc", "done")]),
            ]);
            return out;
        }
        if let Some(e) = &self.err {
            out.push(trunc(
                line([sp(format!("rush serve didn't answer: {e}"), RED)]),
                w,
            ));
            out.push(Line::new());
            out.push(line([sp(
                "Turn Remote mode on, give it a few seconds, and try again.",
                DIM,
            )]));
        } else if self.p.code.is_empty() {
            out.push(line([sp("asking rush serve for a code…", DIM)]));
        } else {
            let code = pair_code(&self.p.code);
            let or = if self.p.url.is_empty() { "" } else { "or " };
            let mut ssh = Vec::new();
            if !self.p.ssh.is_empty() {
                ssh = vec![
                    Line::new(),
                    center_line(
                        line([
                            sp(format!("{or}from an SSH app:  "), DIM),
                            plain(self.p.ssh.clone()),
                        ]),
                        w,
                    ),
                    center_line(line([sp("then type the code when it asks", DIM)]), w),
                    center_line(
                        line([sp(format!("this Mac's key: {}", self.p.host_key), DIM)]),
                        w,
                    ),
                ];
            }
            if self.p.url.is_empty() {
                let local = m
                    .kept
                    .peek::<State>()
                    .is_some_and(|s| s.remote.cfg.reach.is_empty());
                let (why, fix) = if local {
                    (
                        "Serve is reached from this Mac only,",
                        "turn on Over Tailscale to pair one.",
                    )
                } else {
                    (
                        "Not on your tailnet yet,",
                        "sign in to Tailscale on this Mac.",
                    )
                };
                out.push(center_line(
                    line([sp(format!("{why} so no QR code for a phone:"), ORANGE)]),
                    w,
                ));
                out.push(center_line(line([sp(fix, DIM)]), w));
            } else {
                let qr = qr_lines(&self.p.url);
                if qr.len() + ssh.len() + 7 > h {
                    out.push(line([sp(
                        "Make the window taller for the QR code; meanwhile open",
                        DIM,
                    )]));
                    out.push(line([plain(self.p.url.clone())]));
                } else {
                    out.extend(qr.into_iter().map(|l| center_line(l, w)));
                }
            }
            out.extend(ssh);
            let left =
                ((parse_time(&self.p.expires) - m.now).max(0) as f64 / 60_000.0).round() as i64;
            out.push(Line::new());
            out.push(center_line(
                line([
                    sp("Code ", DIM),
                    sp(code, ORANGE),
                    sp(
                        format!(" · works once, for {left} min · waiting for the device…"),
                        DIM,
                    ),
                ]),
                w,
            ));
        }
        out.push(Line::new());
        out.push(keys_fit(w, &[("enter", "a new code"), ("esc", "close")]));
        out
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        match k.s.as_str() {
            "esc" | "ctrl+c" | "q" => Out::Close,
            "enter" | "r" => {
                open_pair_sheet(m);
                Out::Draw
            }
            _ => Out::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(url: &str) -> PairSheet {
        PairSheet {
            p: Pairing {
                code: "ABCD2345".into(),
                url: url.into(),
                expires: String::new(),
                ssh: "ssh -p 2222 rush@mac".into(),
                host_key: "SHA256:xyz".into(),
            },
            err: None,
            opened: 0,
            paired: None,
            gone: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The link as a QR code where there's room, the link itself where
    /// there isn't, and the code under either (pairSheet.body).
    #[test]
    fn pair_sheet_draws_the_qr_code() {
        let m = Model::new("0.1.0");
        let url = "https://rush.example.ts.net:8443/pair#ABCD2345";
        let qr = qr_lines(url);
        assert_eq!(
            qr.len(),
            23,
            "a 41-module code and its quiet zone, two to a row"
        );
        assert!(qr.iter().all(|l| l.width() == 45));
        let tall = sheet(url).body(&m, 66, 60);
        let text: Vec<String> = tall.iter().map(Line::plain).collect();
        assert_eq!(text.iter().filter(|l| l.contains('▀')).count(), 23);
        assert!(
            text.iter()
                .any(|l| l.contains("or from an SSH app:  ssh -p 2222 rush@mac"))
        );
        assert!(
            text.iter()
                .any(|l| l.contains("Code ABCD-2345 · works once"))
        );
        assert!(tall.iter().all(|l| l.width() <= 66));
        let short = sheet(url).body(&m, 66, 20);
        let text: Vec<String> = short.iter().map(Line::plain).collect();
        assert!(!text.iter().any(|l| l.contains('▀')));
        assert!(
            text.iter()
                .any(|l| l.contains("Make the window taller for the QR code"))
        );
        assert!(text.iter().any(|l| l == url));
        let none = sheet("").body(&m, 66, 60);
        let text: Vec<String> = none.iter().map(Line::plain).collect();
        assert!(
            text.iter()
                .any(|l| l.contains("from an SSH app:") && !l.contains("or from"))
        );
    }

    /// Serve's state and each device's last sighting in Go's colours:
    /// running green and the counts dim, a sighting dim (remoteSections,
    /// deviceRow).
    #[test]
    fn serve_and_devices_in_colour() {
        let mut m = Model::new("0.1.0");
        {
            let r = &mut m.kept::<State>().remote;
            r.devs = vec![Device {
                id: "d1".into(),
                kind: "web".into(),
                name: "iPhone".into(),
                ..Device::default()
            }];
        }
        let secs = sections(&m);
        let serve = &secs[0].rows[1];
        assert_eq!(serve.value, "running · 1 paired, 0 connected now");
        assert_eq!(serve.styled.spans[0].style.fg, GREEN);
        assert_eq!(serve.styled.spans[1].style.fg, DIM);
        let dev = &secs[1].rows[0];
        assert_eq!(dev.value, "seen never");
        assert_eq!(dev.styled.spans[0].style.fg, DIM);
        let row = super::super::form::setting_row(false, dev, 20, 12, 80);
        assert!(
            row.spans
                .iter()
                .any(|s| s.text == "seen never" && s.style.fg == DIM)
        );
    }
}
