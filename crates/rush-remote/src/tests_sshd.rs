//! serve's SSH, with this Mac's own OpenSSH client against it on
//! loopback: pairing a key with a code, getting back to the rush it left,
//! revoking it, and each kind of key. After Go's sshd_test.go
//! (TestSSHPairsKeysAndKeepsTheirRush :40). The rush it runs is a shell
//! script standing in for it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::ctx::Ctx;
use crate::http::Request;
use crate::pair::pair_code;
use crate::server::Server;
use crate::sshd::set_ssh_exe;
use crate::testutil::home;

const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SSH: &str = "/usr/bin/ssh";

/// Waits for ok, at most 20s: what it waits on is a process of its own.
fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if ok() {
            return;
        }
        std::thread::park_timeout(Duration::from_millis(20));
    }
    panic!("timed out: {what}");
}

/// A serve with SSH on a loopback port of its own, and that port.
fn serve() -> (Arc<Server>, u16, Ctx) {
    let mut s = Server::new(
        Config {
            name: "studio".into(),
            ..Config::default()
        },
        TOKEN.into(),
        rush_core::adapter::Registry::new(),
    );
    s.bad_guess = Duration::ZERO;
    let s = Arc::new(s);
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = ln.local_addr().unwrap().port();
    let ctx = Ctx::new();
    let (s2, c2) = (s.clone(), ctx.clone());
    std::thread::spawn(move || s2.serve_ssh_on(&c2, ln));
    (s, port, ctx)
}

/// A client: its key, and the code it answers with.
struct Client {
    dir: PathBuf,
    key: PathBuf,
}

impl Client {
    fn new(dir: &Path, kind: &str) -> Client {
        let key = dir.join(format!("id_{kind}"));
        let st = Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", kind, "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap();
        assert!(st.success());
        let ask = dir.join("ask");
        std::fs::write(&ask, "#!/bin/sh\ncat \"$RUSH_TEST_CODE\"\n").unwrap();
        std::fs::set_permissions(&ask, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        Client {
            dir: dir.to_path_buf(),
            key,
        }
    }

    /// ssh to serve on port: with a code to answer, or keys only.
    fn cmd(&self, port: u16, code: Option<&str>) -> Command {
        let codef = self.dir.join("code");
        std::fs::write(&codef, code.unwrap_or("")).unwrap();
        let mut c = Command::new(SSH);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.dir)
            .env("TERM", "xterm-256color")
            .env("SSH_ASKPASS", self.dir.join("ask"))
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("RUSH_TEST_CODE", &codef)
            .args(["-F", "/dev/null", "-i"])
            .arg(&self.key)
            .args(["-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none"])
            .arg("-o")
            .arg(format!(
                "UserKnownHostsFile={}",
                self.dir.join("known_hosts").display()
            ))
            .args([
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "LogLevel=ERROR",
                "-p",
                &port.to_string(),
            ]);
        if code.is_none() {
            c.args(["-o", "PreferredAuthentications=publickey"]);
        }
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// An SSH session's terminal: what it shows, and its keys.
    fn shell(&self, port: u16, code: Option<&str>) -> Shell {
        self.shell_with(port, code, &[])
    }

    fn shell_with(&self, port: u16, code: Option<&str>, extra: &[&str]) -> Shell {
        let mut c = self.cmd(port, code);
        c.args(extra).args(["-tt", "me@127.0.0.1"]);
        let mut child = c.spawn().unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        for mut r in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let out = out.clone();
            std::thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = r.read(&mut b) {
                    if n == 0 {
                        return;
                    }
                    out.lock().unwrap().extend_from_slice(&b[..n]);
                }
            });
        }
        let stdin = child.stdin.take();
        Shell { child, out, stdin }
    }

    /// Whether ssh gets in, running a command (which serve refuses with
    /// why), and what it said; extra are ssh's options.
    fn run_with(&self, port: u16, code: Option<&str>, extra: &[&str]) -> (bool, String) {
        let mut c = self.cmd(port, code);
        c.args(extra)
            .args(["me@127.0.0.1", "ls"])
            .stdin(Stdio::null());
        let o = c.output().unwrap();
        let said = String::from_utf8_lossy(&o.stderr).into_owned();
        let got_in = said.contains("rush over SSH is rush itself: ssh without a command.");
        assert!(!got_in || o.status.code() == Some(1), "{said}");
        (got_in, said)
    }

    fn run(&self, port: u16, code: Option<&str>) -> (bool, String) {
        self.run_with(port, code, &[])
    }
}

struct Shell {
    child: Child,
    out: Arc<Mutex<Vec<u8>>>,
    stdin: Option<ChildStdin>,
}

impl Shell {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }
    fn shows(&self, want: &str) {
        until(&format!("the screen to show {want:?}"), || {
            self.text().contains(want)
        });
    }
    fn send(&mut self, s: &str) {
        let w = self.stdin.as_mut().unwrap();
        w.write_all(s.as_bytes()).unwrap();
        w.flush().unwrap();
    }
    /// Whether ssh ended, and how.
    fn ended(&mut self) -> Option<bool> {
        self.child.try_wait().unwrap().map(|s| s.success())
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The stand-in for rush: says it's ready, redraws on a resize, and
/// echoes what's typed.
fn fake_rush(dir: &Path) -> PathBuf {
    let exe = dir.join("rush");
    std::fs::write(
        &exe,
        "#!/bin/sh\nprintf '\\033[?1049h'\necho \"ready $$\"\ntrap 'echo redraw' WINCH\nwhile :; do read l && echo \"got $l\"; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    exe
}

/// A key that isn't paired is asked for a code, and the code pairs it; it
/// then gets in with no code, to the same rush it left (which is told
/// its screen modes again and redraws), and revoking it ends that rush.
#[test]
fn ssh_pairs_keys_and_keeps_their_rush() {
    let _h = home();
    let d = tempfile::tempdir().unwrap();
    set_ssh_exe(Some(fake_rush(d.path())));
    let (s, port, ctx) = serve();
    let c = Client::new(d.path(), "ed25519");

    let (ok, _) = c.run(port, None);
    assert!(!ok, "an unpaired key got in");
    let (ok, _) = c.run(port, Some("WRONGCOD"));
    assert!(!ok, "a wrong code got in");
    assert!(s.devs.all().is_empty());

    let (code, _) = s.pairs.issue();
    let mut sh = c.shell(port, Some(&pair_code(&code).to_lowercase()));
    sh.shows("ready ");
    let t = sh.text();
    let pid = t[t.find("ready ").unwrap()..]
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    sh.send("one\r");
    sh.shows("got one");
    let devs = s.devs.all();
    assert_eq!(devs.len(), 1, "{devs:?}");
    assert_eq!(devs[0].kind, "ssh");
    assert!(devs[0].name.starts_with("OpenSSH · "), "{}", devs[0].name);
    assert!(
        devs[0].key.starts_with("ssh-ed25519 AAAA"),
        "{}",
        devs[0].key
    );
    assert!(devs[0].fingerprint.starts_with("SHA256:"));
    let id = devs[0].id.clone();
    until("the connection to count", || s.live.count(&id) == 1);
    drop(sh);
    until("the connection to go", || s.live.count(&id) == 0);
    assert_eq!(s.tuis.len(), 1, "its rush is kept");

    let mut sh = c.shell(port, None);
    sh.shows("\x1b[?1049h");
    sh.shows("redraw");
    sh.send("two\r");
    sh.shows("got two");
    assert!(
        !sh.text().contains("ready"),
        "a new rush, not the one kept (was {pid}): {:?}",
        sh.text()
    );

    let r = Request::new("POST", &format!("/api/devices/{id}/revoke"))
        .with_body("{}")
        .with_header("Host", "rush.test")
        .with_header("Authorization", &format!("Bearer {TOKEN}"))
        .with_header("Content-Type", "application/json");
    assert_eq!(s.handler()(&r).status, 200);
    until("its rush to end", || s.tuis.len() == 0);
    until("the connection to end", || sh.ended().is_some());
    let (ok, _) = c.run(port, None);
    assert!(!ok, "a revoked key got in");

    ctx.cancel();
    set_ssh_exe(None);
}

/// RSA and ECDSA keys pair as ed25519 ones do; a command is refused with
/// why, and a second connection takes rush from the first.
#[test]
fn other_keys_commands_and_a_second_connection() {
    let _h = home();
    let d = tempfile::tempdir().unwrap();
    set_ssh_exe(Some(fake_rush(d.path())));
    let (s, port, ctx) = serve();
    for kind in ["rsa", "ecdsa"] {
        let kd = d.path().join(kind);
        std::fs::create_dir_all(&kd).unwrap();
        let c = Client::new(&kd, kind);
        let (code, _) = s.pairs.issue();
        let (ok, said) = c.run(port, Some(&code));
        assert!(ok, "{kind}: {said}");
        // Paired now: in with the key alone, with each cipher.
        for cipher in [
            "chacha20-poly1305@openssh.com",
            "aes256-gcm@openssh.com",
            "aes128-gcm@openssh.com",
        ] {
            let (ok, said) = c.run_with(port, None, &["-c", cipher]);
            assert!(ok, "{kind} {cipher}: {said}");
        }
    }
    assert_eq!(s.devs.all().len(), 2);

    let c = Client::new(d.path(), "ed25519");
    let (code, _) = s.pairs.issue();
    let mut one = c.shell(port, Some(&code));
    one.shows("ready ");
    let mut two = c.shell_with(port, None, &["-o", "RekeyLimit=1K"]);
    two.shows("redraw");
    one.shows("rush moved to another connection.");
    until("the first to end", || one.ended().is_some());
    // Past 1K the client changes keys, again and again, mid-session.
    for i in 0..40 {
        two.send(&format!("line {i} {}\r", "x".repeat(100)));
    }
    two.shows("got line 39 ");

    ctx.cancel();
    s.tuis.end_all();
    until("its rush to end", || s.tuis.len() == 0);
    set_ssh_exe(None);
}
