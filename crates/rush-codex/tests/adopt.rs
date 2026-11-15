//! A Codex thread handed on, its app-server running on, as a host exec'd
//! onto a newer rush takes it up (Rust-only): against a fake app-server,
//! a shell script speaking just enough of the protocol over its pipes.

// Every test runs in a throwaway home (rush-testenv).
rush_testenv::sandboxed!();

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use rush_codex::Codex;
use rush_core::adapter::{
    Adopter, Conn, Driver, Input, Profile, Said, StartOptions, Tagged, Upgrade,
};
use rush_core::event::Event;

/// The fake app-server. Each turn's reply says which process answered and
/// the turn/start's request id, and whether that id came before; a turn
/// asked to HANG never ends, one asked to ASK waits on an approval.
const FAKE: &str = r#"n=0
seen=" "
while IFS= read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/^{"id":\([0-9]*\),.*/\1/p')
  m=$(printf '%s' "$l" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  dup=new
  case "$seen" in *" $id "*) dup=dup ;; esac
  [ -n "$id" ] && seen="$seen$id "
  case "$m" in
  initialize) echo '{"id":'"$id"',"result":{"userAgent":"rush/0.200.0 (test)"}}' ;;
  thread/start) echo '{"id":'"$id"',"result":{"thread":{"id":"th1"},"model":"gpt-test","cwd":"/w","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}}' ;;
  turn/start)
    n=$((n+1))
    echo '{"id":'"$id"',"result":{"turn":{"id":"t'"$n"'"}}}'
    echo '{"method":"turn/started","params":{"threadId":"th1","turn":{"id":"t'"$n"'"}}}'
    case "$l" in
    *HANG*) ;;
    *ASK*) echo '{"id":"a'"$n"'","method":"item/commandExecution/requestApproval","params":{"threadId":"th1","turnId":"t'"$n"'","itemId":"c'"$n"'","command":"ls","cwd":"/w"}}' ;;
    *)
      echo '{"method":"item/completed","params":{"threadId":"th1","turnId":"t'"$n"'","item":{"type":"agentMessage","id":"m'"$n"'","text":"<pid='"$$"' id='"$id"' '"$dup"'>"}}}'
      echo '{"method":"turn/completed","params":{"threadId":"th1","turn":{"id":"t'"$n"'","status":"completed"}}}'
      ;;
    esac ;;
  "") ;;
  *) [ -n "$id" ] && echo '{"id":'"$id"',"error":{"code":-32000,"message":"not here"}}' ;;
  esac
done
"#;

/// A Codex thread on the fake, started, and what it says.
struct Run {
    _dir: tempfile::TempDir,
    o: StartOptions,
    rx: Receiver<Said<()>>,
    conn: Box<dyn Conn>,
}

fn start() -> Run {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("codex");
    std::fs::write(&bin, format!("#!/bin/sh\n{FAKE}")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let o = StartOptions {
        profile: Profile {
            kind: "codex".into(),
            name: "test".into(),
            dir: home.to_string_lossy().into_owned(),
        },
        dir: dir.path().to_string_lossy().into_owned(),
        binary: bin.to_string_lossy().into_owned(),
        ..StartOptions::default()
    };
    let (tx, rx) = channel();
    let conn = Codex::new()
        .start(o.clone(), Arc::new(Tagged { key: (), tx }))
        .unwrap();
    Run {
        _dir: dir,
        o,
        rx,
        conn,
    }
}

impl Run {
    /// Events until one f picks, which it returns.
    fn until<T>(&self, mut f: impl FnMut(&Event) -> Option<T>) -> T {
        loop {
            match self.rx.recv_timeout(Duration::from_secs(10)) {
                Ok(Said::Event(_, e)) => {
                    if let Some(t) = f(&e) {
                        return t;
                    }
                }
                Ok(Said::Ended(_, e)) => panic!("ended: {e:?}"),
                Err(e) => panic!("no event: {e}"),
            }
        }
    }

    /// Sends text and waits for the turn's reply: the pid and request id
    /// it names.
    fn turn(&self, text: &str) -> (i32, i64) {
        self.conn
            .send(Input {
                text: text.into(),
                ..Input::default()
            })
            .unwrap();
        let said = self.until(|e| {
            let d = format!("{e:?}");
            let at = d.find("<pid=")?;
            let end = d[at..].find('>')?;
            Some(d[at + 1..at + end].to_string())
        });
        self.until(|e| matches!(e, Event::TurnEnd(_)).then_some(()));
        let f: Vec<&str> = said.split(' ').collect();
        assert_eq!(f.get(2), Some(&"new"), "a request id used twice: {said}");
        let num = |s: &str, k: &str| s.strip_prefix(k).unwrap().parse::<i64>().unwrap();
        (num(f[0], "pid=") as i32, num(f[1], "id="))
    }

    fn pass(&self) -> Upgrade {
        self.conn
            .upgrader()
            .expect("codex upgrades")
            .pass()
            .unwrap()
    }
}

/// Passed between turns, kept (the exec didn't happen), passed again and
/// taken up on the same pipes: the same app-server answers the next turn,
/// told nothing new, its request ids going on from the last build's.
#[test]
fn passed_and_taken_up_the_same_app_server_answers() {
    let mut run = start();
    let pid = run.conn.pid().expect("a process of its own");
    let (p1, id1) = run.turn("hi");
    assert_eq!(p1, pid);

    // The exec failed: it carries on as it was.
    run.pass();
    run.conn.upgrader().unwrap().keep();
    let (p2, id2) = run.turn("still here");
    assert_eq!(p2, pid);
    assert!(id2 > id1);

    let u = run.pass();
    assert_eq!(u.pid, pid);
    for f in ["stdin", "stdout"] {
        assert!(u.fds.contains_key(f), "{f} in {:?}", u.fds);
    }
    assert_eq!(u.state["thread"], "th1");
    // Through JSON, as the host writes it.
    let u: Upgrade = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
    let (tx, rx) = channel();
    let adopted = Codex::new()
        .adopt(run.o.clone(), u, Arc::new(Tagged { key: (), tx }))
        .unwrap();
    run.rx = rx;
    // The exec: the old build's conn is gone without a word to the
    // app-server, its files left open for the new one.
    std::mem::forget(std::mem::replace(&mut run.conn, adopted));
    assert_eq!(run.conn.pid(), Some(pid));
    let (p3, id3) = run.turn("again");
    assert_eq!(p3, pid, "another process answered");
    assert!(id3 > id2, "ids went back: {id3} after {id2}");
    run.conn.close().unwrap();
}

/// A thread isn't handed on while it works or waits on your answer: the
/// host is told why and stays put, the thread as it was.
#[test]
fn refuses_mid_turn_and_with_an_approval_open() {
    for (text, why) in [
        ("HANG", "in the middle of a turn"),
        ("ASK", "waiting on your answer"),
    ] {
        let run = start();
        run.conn
            .send(Input {
                text: text.into(),
                ..Input::default()
            })
            .unwrap();
        match text {
            "ASK" => run.until(|e| matches!(e, Event::Approval(_)).then_some(())),
            _ => run.until(|e| matches!(e, Event::Status(s) if s.busy).then_some(())),
        }
        let e = run.conn.upgrader().unwrap().pass().unwrap_err();
        assert!(e.to_string().contains(why), "{text}: {e}");
        run.conn.close().unwrap();
    }
}
