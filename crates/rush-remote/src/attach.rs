//! Another machine's sessions as part of this machine's rush: each shows in
//! the list and opens in the view like a local one, and is driven through
//! that machine's serve. It stands in for their hosts: for every remote
//! session it keeps a session folder, with an info file and a host socket,
//! that speaks the host protocol on one side and serve's HTTP on the
//! other. Nothing runs here: the agent, its files and its tools stay on
//! the other machine.
//!
//! What it can't be, it says: only the ops serve allows pass; each
//! stand-in carries Info.remote, which the view and the host module check
//! before any local action; its working folder is under a folder of
//! rush's own that nothing can enter (unreachable), so a path that also
//! exists here is never the session's; and history is what serve replays.
//! A folder is its own only if it holds the marker it wrote.
//!
//! After Go's internal/remote/attach.go (Attach :46, attachPeers :84,
//! alias :137, marker :150, localID :172, unreachable :182, localInfo
//! :207, readMarker :227, purge :242, owns :262, drop :268, reserve :286,
//! sync :325, validRemoteID :372, serve :377, conn :391, op :447,
//! readPictures :488, hostError :519, hostLine :527).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_host::Info;
use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::client::{peer_get, status_line};
use crate::config::{Config, Peer, current, dir, hex, or, rand_bytes};
use crate::ctx::Ctx;
use crate::expose::{Log, fields, say};
use crate::git::unreachable;
use crate::server::{ErrorBody, Picture, REMOTE_OPS, RemoteOp, Session};

/// bridge.json in an alias's folder: the proof an attach made the folder,
/// for which machine and session, and which attach (pid) keeps it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Marker {
    machine: String,
    #[serde(rename = "remoteId")]
    remote_id: String,
    pid: i64,
    token: String,
}

const MARKER: &str = "bridge.json";

/// The n-th id a remote session may have here: eight hex digits, from the
/// machine, its own id and n.
pub fn local_id(machine: &str, rid: &str, n: u32) -> String {
    hex(&Sha256::digest(format!("{machine}/{rid}/{n}").as_bytes())[..4])
}

fn make_unreachable(machine: &str) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let d = unreachable(machine);
    if let Some(p) = d.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(p)
            .map_err(|e| e.to_string())?;
    }
    match std::fs::DirBuilder::new().mode(0o000).create(&d) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e.to_string()),
        _ => {}
    }
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000)).map_err(|e| e.to_string())
}

fn remove_unreachable(machine: &str) {
    use std::os::unix::fs::PermissionsExt;
    let d = unreachable(machine);
    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::remove_dir(&d);
}

/// A remote session's info as this machine shows it, as the stand-in with
/// id: marked remote, with the attach's pid, and nothing of the agent's
/// own processes or folders.
pub fn local_info(machine: &str, id: &str, pid: i64, mut i: Info) -> Info {
    let rid = std::mem::take(&mut i.id);
    i.id = id.into();
    i.remote = machine.into();
    i.remote_id = rid.clone();
    i.host_pid = pid;
    i.claude_pid = 0;
    // Waking is the remote's, on a send.
    i.sleeping = false;
    let joined = format!(
        "{}/{}",
        unreachable(machine).display(),
        i.cwd.trim_start_matches('/')
    );
    i.cwd = crate::server::clean(Path::new(&joined))
        .to_string_lossy()
        .into_owned();
    i.cwd_at = Time::default();
    i.temp_dir.clear();
    i.left.clear();
    if i.name.is_empty() {
        i.name = rid;
    }
    i.name = format!("{} @{machine}", i.name);
    i
}

/// dir's marker, if dir is a real folder (not a link) that has one.
fn read_marker(d: &Path) -> Option<Marker> {
    let st = std::fs::symlink_metadata(d).ok()?;
    if !st.is_dir() {
        return None;
    }
    let m: Marker = serde_json::from_slice(&std::fs::read(d.join(MARKER)).ok()?).ok()?;
    (!m.token.is_empty()).then_some(m)
}

/// One remote session as a local one.
struct Alias {
    id: String,
    rid: String,
    dir: PathBuf,
    token: String,
    /// Ends its accepting, and its views' connections.
    stop: Ctx,
    wake: Option<UnixStream>,
    /// info.json as last written.
    last: Vec<u8>,
}

impl Alias {
    /// Whether its folder is still the one it made.
    fn owns(&self) -> bool {
        read_marker(&self.dir).is_some_and(|m| m.token == self.token)
    }
}

struct Bridge {
    p: Peer,
    log: Log,
    pid: i64,
    ctx: Ctx,
    sess: Mutex<HashMap<String, Alias>>,
}

/// Makes p's sessions part of this machine's rush until ctx ends, then
/// removes what it made, and only that.
pub fn attach(ctx: &Ctx, p: &Peer, log: &Log) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    // One attach per machine: a second would take over the first's sockets.
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir().join(format!("attach-{}.lock", p.name)))
        .map_err(|e| e.to_string())?;
    // SAFETY: flock on a file this holds open for the attach's life.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("already attached to {} by another rush", p.name));
    }
    make_unreachable(&p.name)?;
    std::fs::create_dir_all(rush_host::root()).map_err(|e| e.to_string())?;
    let b = Arc::new(Bridge {
        p: p.clone(),
        log: log.clone(),
        pid: std::process::id() as i64,
        ctx: ctx.child(),
        sess: Mutex::new(HashMap::new()),
    });
    b.purge();
    loop {
        if let Err(e) = b.sync()
            && !ctx.done()
        {
            say(log, &format!("rush remote attach: {}: {e}\n", p.name));
        }
        if !ctx.sleep(Duration::from_secs(3)) {
            break;
        }
    }
    b.close();
    remove_unreachable(&p.name);
    drop(lock);
    Ok(())
}

/// Runs attach for each of cfg's peers while remote.json's attachPeers is
/// on; off, or once ctx ends, it ends them. A peer whose attach ends is
/// tried again a little later.
pub fn attach_peers(ctx: &Ctx, cfg: &Config, log: &Log) {
    struct Running {
        stop: Ctx,
        done: std::sync::mpsc::Receiver<()>,
        finished: bool,
    }
    let mut on: HashMap<String, Running> = HashMap::new();
    let mut retry: HashMap<String, Instant> = HashMap::new();
    loop {
        let want = !ctx.done() && current().attach_peers;
        for p in &cfg.peers {
            if let Some(r) = on.get_mut(&p.name)
                && (r.finished || r.done.try_recv().is_ok())
            {
                r.finished = true;
                on.remove(&p.name);
                retry.insert(p.name.clone(), Instant::now() + Duration::from_secs(30));
            }
            let running = on.contains_key(&p.name);
            let due = retry.get(&p.name).is_none_or(|t| Instant::now() > *t);
            if want && !running && due {
                let c = ctx.child();
                let (tx, rx) = std::sync::mpsc::channel();
                rush_events::log("attach-started", fields(json!({ "peer": p.name })));
                let (pc, lc, cc) = (p.clone(), log.clone(), c.clone());
                std::thread::spawn(move || {
                    if let Err(e) = attach(&cc, &pc, &lc)
                        && !cc.done()
                    {
                        rush_events::log(
                            "attach-failed",
                            fields(
                                json!({ "peer": pc.name, "err": e, "why": "tried again in 30s" }),
                            ),
                        );
                        say(&lc, &format!("rush serve: attach {}: {e}\n", pc.name));
                    }
                    let _ = tx.send(());
                });
                on.insert(
                    p.name.clone(),
                    Running {
                        stop: c,
                        done: rx,
                        finished: false,
                    },
                );
            } else if !want && running {
                rush_events::log(
                    "attach-stopped",
                    fields(
                        json!({ "peer": p.name, "why": "attachPeers is off, or serve is ending" }),
                    ),
                );
                if let Some(r) = on.remove(&p.name) {
                    r.stop.cancel();
                    let _ = r.done.recv();
                }
            }
        }
        if ctx.done() {
            return;
        }
        ctx.sleep(Duration::from_secs(3));
    }
}

impl Bridge {
    /// Removes what an attach to this machine that didn't end cleanly
    /// left: folders with this machine's marker of an attach not running.
    fn purge(&self) {
        let Ok(ents) = std::fs::read_dir(rush_host::root()) else {
            return;
        };
        for e in ents.flatten() {
            let d = e.path();
            if let Some(m) = read_marker(&d)
                && m.machine == self.p.name
                && m.pid != self.pid
                && !rush_host::alive(m.pid)
            {
                let _ = std::fs::remove_dir_all(&d);
            }
        }
    }

    fn close(&self) {
        let mut s = self.sess.lock().unwrap_or_else(|e| e.into_inner());
        for (_, a) in s.drain() {
            drop_alias(a);
        }
    }

    /// Makes a folder for remote session rid under the first id nobody
    /// has. Making the folder is the claim: it fails if anything is there,
    /// so nothing of anyone's is touched; a folder with this machine's
    /// marker of an attach that has ended is its own stale one, taken back.
    fn reserve(self: &Arc<Self>, rid: &str) -> Result<Alias, String> {
        use std::os::unix::fs::DirBuilderExt;
        let mk = |d: &Path| std::fs::DirBuilder::new().mode(0o700).create(d);
        for n in 0..100 {
            let id = local_id(&self.p.name, rid, n);
            let d = rush_host::root().join(&id);
            match mk(&d) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let ours = read_marker(&d).is_some_and(|m| {
                        m.machine == self.p.name
                            && m.remote_id == rid
                            && m.pid != self.pid
                            && !rush_host::alive(m.pid)
                    });
                    if !ours || std::fs::remove_dir_all(&d).is_err() || mk(&d).is_err() {
                        // Somebody's: take the next id.
                        continue;
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
            let token = hex(&rand_bytes(16));
            let raw = serde_json::to_vec(&Marker {
                machine: self.p.name.clone(),
                remote_id: rid.into(),
                pid: self.pid,
                token: token.clone(),
            })
            .unwrap_or_default();
            if let Err(e) = crate::config::write_private(&d.join(MARKER), &raw) {
                let _ = std::fs::remove_dir_all(&d);
                return Err(e);
            }
            let ln = match UnixListener::bind(rush_host::sock_path(&id)) {
                Ok(l) => l,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&d);
                    return Err(e.to_string());
                }
            };
            let stop = self.ctx.child();
            let (wake_r, wake_w) = UnixStream::pair().map_err(|e| e.to_string())?;
            self.serve(ln, wake_r, &stop, rid);
            return Ok(Alias {
                id,
                rid: rid.into(),
                dir: d,
                token,
                stop,
                wake: Some(wake_w),
                last: Vec::new(),
            });
        }
        Err("no free session id".into())
    }

    /// Reads the remote's sessions and brings the local folders to match.
    fn sync(self: &Arc<Self>) -> Result<(), String> {
        let list: Vec<Session> =
            peer_get::<Option<Vec<Session>>>(&self.p, "/api/sessions", Duration::from_secs(15))?
                .unwrap_or_default();
        let mut sess = self.sess.lock().unwrap_or_else(|e| e.into_inner());
        let mut seen = std::collections::HashSet::new();
        for s in list {
            if !valid_remote_id(&s.info.id) {
                continue;
            }
            let rid = s.info.id.clone();
            seen.insert(rid.clone());
            if !sess.contains_key(&rid) {
                match self.reserve(&rid) {
                    Ok(a) => {
                        sess.insert(rid.clone(), a);
                    }
                    Err(e) => {
                        say(&self.log, &format!("rush remote attach: {rid}: {e}\n"));
                        continue;
                    }
                }
            }
            let Some(a) = sess.get_mut(&rid) else {
                continue;
            };
            let mut info = local_info(&self.p.name, &a.id, self.pid, s.info);
            if !s.alive {
                info.state = "stopped".into();
            }
            let raw = serde_json::to_vec(&info).unwrap_or_default();
            if raw == a.last {
                continue;
            }
            if !a.owns() {
                // The folder was replaced under us: touch nothing in it.
                say(
                    &self.log,
                    &format!(
                        "rush remote attach: {}'s folder is no longer ours; leaving it\n",
                        a.id
                    ),
                );
                if let Some(a) = sess.remove(&rid) {
                    drop_alias(a);
                }
                continue;
            }
            let tmp = a.dir.join("info.json.tmp");
            if std::fs::write(&tmp, &raw).is_ok()
                && std::fs::rename(&tmp, a.dir.join("info.json")).is_ok()
            {
                a.last = raw;
            }
        }
        let gone: Vec<String> = sess
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        for rid in gone {
            if let Some(a) = sess.remove(&rid) {
                drop_alias(a);
            }
        }
        Ok(())
    }

    /// Answers on an alias's socket until stop: a view's connections, each
    /// on a thread of its own. Waits on the socket and the wake together,
    /// so nothing polls.
    fn serve(self: &Arc<Self>, ln: UnixListener, wake: UnixStream, stop: &Ctx, rid: &str) {
        let (b, stop, rid) = (self.clone(), stop.clone(), rid.to_string());
        let id_dir = String::new();
        let _ = id_dir;
        std::thread::spawn(move || {
            loop {
                let mut fds = [
                    libc::pollfd {
                        fd: ln.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: poll on two fds this thread holds open.
                let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                if r < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if fds[1].revents != 0 || stop.done() {
                    return;
                }
                if fds[0].revents & libc::POLLIN == 0 {
                    continue;
                }
                let Ok((c, _)) = ln.accept() else {
                    continue;
                };
                let (b, stop, rid) = (b.clone(), stop.child(), rid.clone());
                std::thread::spawn(move || b.conn(&stop, &rid, c));
            }
        });
    }

    /// One view's connection to a session: what the remote streams goes to
    /// it as host lines, what it sends goes to the remote as ops.
    fn conn(self: &Arc<Self>, ctx: &Ctx, rid: &str, c: UnixStream) {
        let Ok(r) = c.try_clone() else {
            return;
        };
        let w = Arc::new(Mutex::new(c));
        let write = {
            let w = w.clone();
            move |line: &[u8]| -> bool {
                let mut g = w.lock().unwrap_or_else(|e| e.into_inner());
                let mut b = line.to_vec();
                b.push(b'\n');
                g.write_all(&b).is_ok()
            }
        };
        let id = {
            let s = self.sess.lock().unwrap_or_else(|e| e.into_inner());
            s.get(rid).map(|a| a.id.clone()).unwrap_or_default()
        };
        {
            // The view's ops.
            let (b, ctx, rid, write) = (self.clone(), ctx.clone(), rid.to_string(), write.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(r);
                let mut l = Vec::new();
                loop {
                    l.clear();
                    match r.read_until(b'\n', &mut l) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let line = l.strip_suffix(b"\n").unwrap_or(&l);
                    if let Err(e) = b.op(&rid, line) {
                        write(&host_error(&e));
                    }
                }
                ctx.cancel();
            });
        }
        // A view takes all of it, as it would a local transcript.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let Ok(resp) = agent
            .get(&format!("{}/api/sessions/{rid}/events?full=1", self.p.url))
            .header("Authorization", &format!("Bearer {}", self.p.token))
            .call()
        else {
            return;
        };
        if resp.status().as_u16() != 200 {
            return;
        }
        let mut rd = BufReader::new(resp.into_body().into_reader());
        let mut resets = 0;
        let mut l = String::new();
        loop {
            if ctx.done() {
                break;
            }
            l.clear();
            match rd.read_line(&mut l) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Some(data) = l.trim_end_matches(['\r', '\n']).strip_prefix("data: ") else {
                continue;
            };
            let (out, reset) = host_line(&self.p.name, &id, self.pid, data.as_bytes());
            if reset {
                // The remote started over: a view that redials gets a clean
                // replay, which is all a view can take.
                resets += 1;
                if resets > 1 {
                    break;
                }
                continue;
            }
            if let Some(out) = out
                && !write(&out)
            {
                break;
            }
        }
        let g = w.lock().unwrap_or_else(|e| e.into_inner());
        let _ = g.shutdown(std::net::Shutdown::Both);
    }

    /// Passes one of the view's host ops to the remote, if serve allows it.
    fn op(&self, rid: &str, line: &[u8]) -> Result<(), String> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Head {
            op: String,
            #[serde(deserialize_with = "rush_core::wire::nullvec::deserialize")]
            images: Vec<String>,
        }
        let h: Head = serde_json::from_slice(line).unwrap_or_default();
        if h.op == "hello" || h.op.is_empty() {
            return Ok(());
        }
        if !REMOTE_OPS.contains(&h.op.as_str()) {
            return Err(format!(
                "{} isn't available on a session on {}",
                h.op, self.p.name
            ));
        }
        // Only what serve takes: never a path on this machine.
        let mut o: RemoteOp = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        o.pictures = read_pictures(&h.images)?;
        let body = serde_json::to_vec(&o).map_err(|e| e.to_string())?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut resp = agent
            .post(&format!("{}/api/sessions/{rid}/op", self.p.url))
            .header("Authorization", &format!("Bearer {}", self.p.token))
            .header("Content-Type", "application/json")
            .send(&body[..])
            .map_err(|e| format!("{} can't be reached: {e}", self.p.name))?;
        let status = resp.status().as_u16();
        if status >= 300 {
            let raw = resp
                .body_mut()
                .with_config()
                .limit(1 << 16)
                .read_to_vec()
                .unwrap_or_default();
            let e: ErrorBody = serde_json::from_slice(&raw).unwrap_or_default();
            return Err(or(&e.error, &status_line(status)));
        }
        Ok(())
    }
}

/// Ends a, removing its folder only if it still owns it.
fn drop_alias(mut a: Alias) {
    a.stop.cancel();
    // Wakes its accept, which ends.
    drop(a.wake.take());
    if a.owns() {
        let _ = std::fs::remove_dir_all(&a.dir);
    }
    let _ = (&a.id, &a.rid);
}

fn valid_remote_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\', '.', '?', '#', '%', ' '])
}

/// The images a view attached, by their paths here, as bytes to send: the
/// path stays on this machine.
fn read_pictures(paths: &[String]) -> Result<Vec<Picture>, String> {
    let mut out = Vec::new();
    for p in paths {
        let base = Path::new(p)
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = Path::new(p)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let kind = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return Err(format!("{base} isn't a png, jpeg, gif or webp image")),
        };
        let st = std::fs::metadata(p).map_err(|e| e.to_string())?;
        const MOST: u64 = 20 << 20;
        if st.len() > MOST {
            return Err(format!(
                "{base} is {} MB; images must be under {} MB",
                st.len() >> 20,
                MOST >> 20
            ));
        }
        let data = std::fs::read(p).map_err(|e| e.to_string())?;
        out.push(Picture {
            kind: kind.into(),
            data,
        });
    }
    Ok(out)
}

fn host_error(msg: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "type": "agtop_error", "error": msg })).unwrap_or_default()
}

/// One of serve's event lines ({"t":…,"e":…}) as the line a host would
/// have sent, None for ones with no use; true says the stream began again.
pub fn host_line(machine: &str, id: &str, pid: i64, data: &[u8]) -> (Option<Vec<u8>>, bool) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct W {
        t: String,
        e: Option<Box<serde_json::value::RawValue>>,
    }
    let Ok(w) = serde_json::from_slice::<W>(data) else {
        return (None, false);
    };
    let e = w.e.as_ref().map_or("null", |r| r.get());
    let get = |k: &str| -> Value {
        let v: Value = serde_json::from_str(e).unwrap_or(Value::Null);
        v.as_object()
            .and_then(|m| {
                m.iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(k))
                    .map(|(_, v)| v.clone())
            })
            .unwrap_or(Value::Null)
    };
    let s = |v: Value| v.as_str().unwrap_or_default().to_string();
    let line = match w.t.as_str() {
        "reset" => return (None, true),
        "cut" | "" => return (None, false),
        "info" => {
            let Ok(i) = serde_json::from_str::<Info>(e) else {
                return (None, false);
            };
            let raw = serde_json::to_vec(&local_info(machine, id, pid, i)).unwrap_or_default();
            let mut b = br#"{"info":"#.to_vec();
            b.extend_from_slice(&raw);
            b.extend_from_slice(br#","type":"agtop_info"}"#);
            b
        }
        "error" => host_error(&s(get("Error"))),
        "answered" => {
            serde_json::to_vec(&json!({ "type": "agtop_answered", "request_id": s(get("ID")) }))
                .unwrap_or_default()
        }
        "stamp" => {
            let Some(t) = Time::parse(&s(get("At"))) else {
                return (None, false);
            };
            serde_json::to_vec(&json!({ "type": "agtop_time", "t": t.to_ms() })).unwrap_or_default()
        }
        "sent" => {
            let mut m = json!({
                "type": "user",
                "message": { "role": "user", "content": s(get("Text")) },
                "agtop_sent": true,
            });
            let ex = get("Exchange");
            if !ex.is_null() {
                m["agtop_exchange"] = ex;
            }
            serde_json::to_vec(&m).unwrap_or_default()
        }
        _ => {
            if rush_core::event::unmarshal(data).is_err() {
                return (None, false);
            }
            let mut b = br#"{"type":"agtop_ev","ev":"#.to_vec();
            b.extend_from_slice(data);
            b.push(b'}');
            b
        }
    };
    (Some(line), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_lines() {
        let (l, reset) = host_line("box", "abcd1234", 7, br#"{"t":"reset","e":null}"#);
        assert!(l.is_none() && reset);
        let (l, _) = host_line("box", "abcd1234", 7, br#"{"e":{"ID":"r1"},"t":"answered"}"#);
        assert_eq!(
            l.unwrap(),
            br#"{"request_id":"r1","type":"agtop_answered"}"#
        );
        let (l, _) = host_line("box", "abcd1234", 7, br#"{"e":{"Error":"no"},"t":"error"}"#);
        assert_eq!(l.unwrap(), br#"{"error":"no","type":"agtop_error"}"#);
        let (l, _) = host_line(
            "box",
            "abcd1234",
            7,
            br#"{"e":{"Exchange":null,"Images":null,"Text":"hi"},"t":"sent"}"#,
        );
        assert_eq!(
            l.unwrap(),
            br#"{"agtop_sent":true,"message":{"content":"hi","role":"user"},"type":"user"}"#
        );
        let (l, _) = host_line("box", "abcd1234", 7, br#"{"t":"cut"}"#);
        assert!(l.is_none());
        let (l, _) = host_line(
            "box",
            "abcd1234",
            7,
            br#"{"e":{"id":"r9","name":"fix","cwd":"/src/x","hostPid":99,"state":"idle"},"t":"info"}"#,
        );
        let l = String::from_utf8(l.unwrap()).unwrap();
        assert!(l.starts_with(r#"{"info":{"id":"abcd1234""#), "{l}");
        assert!(l.ends_with(r#","type":"agtop_info"}"#), "{l}");
        let i: Info = serde_json::from_str(&l[8..l.len() - 21]).unwrap();
        assert_eq!(i.name, "fix @box");
        assert_eq!(i.remote, "box");
        assert_eq!(i.remote_id, "r9");
        assert_eq!(i.host_pid, 7);
        assert!(i.cwd.ends_with("/remote-cwd/box/src/x"), "{}", i.cwd);
        // Rubbish is no line.
        assert!(host_line("box", "x", 7, b"nope").0.is_none());
    }

    #[test]
    fn local_ids_are_eight_hex_digits() {
        let a = local_id("box", "r1", 0);
        assert_eq!(a.len(), 8);
        assert_ne!(a, local_id("box", "r1", 1));
        assert!(valid_remote_id("abc123"));
        assert!(!valid_remote_id("../x"));
    }
}
