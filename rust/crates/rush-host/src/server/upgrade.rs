//! The host moving onto a newer rush in place, its agent running on: the
//! upgrade op, the exec, and the new build taking up where the old left
//! off (see crate::upgrade for the whole of it). Rust-only.

use std::os::fd::{FromRawFd, RawFd};

use rush_core::adapter::{Profile, StartOptions, Upgrade};

use super::*;
use crate::upgrade::{Kept, dec, enc, state_path};

/// Stands in for the exec (exe, args) that moves the host: what it
/// returns is why it didn't happen.
pub type Exec = Arc<dyn Fn(&Path, &[String]) -> std::io::Error + Send + Sync>;

impl Host {
    /// Asked to move onto the installed rush: once it's between turns, at
    /// once when it is. Asked again, it tries again.
    pub(super) fn want_upgrade(&self, st: &mut State) {
        st.upgrade_due = true;
        st.info.upgrade = "due".into();
        self.event(st, "upgrade-asked", json!({"state": st.info.state}));
        self.publish(st); // which moves it, between turns
    }

    /// Between turns, with nothing the move would lose: no message queued,
    /// question asked, task running, retry or limit waiting, nor a start
    /// or stop under way.
    fn may_move(&self, st: &State) -> bool {
        let i = &st.info;
        i.state == "idle"
            && !i.sleeping
            && i.queue.is_empty()
            && i.needs.is_empty()
            && i.background.is_empty()
            && i.tasks == 0
            && i.limit.is_none()
            && i.retry.is_none()
            && i.waking.is_zero()
            && st.pending.is_empty()
            && st.asking == 0
            && !st.stopping
            && !st.quit
    }

    /// Moves the host, on a thread of its own, once a move is due and it's
    /// between turns: what publish asks after each change.
    pub(super) fn upgrade_when_due(&self, st: &mut State) {
        if !st.upgrade_due || st.upgrading || !self.may_move(st) {
            return;
        }
        st.upgrading = true;
        let h = self.clone();
        self.go("upgrade", move |_| h.upgrade());
    }

    /// Asks the installed rush whether it can take the agent up, then
    /// passes everything to it and execs it. Returns only when it didn't.
    fn upgrade(&self) {
        let id = self.lock().cfg.id.clone();
        let exe = crate::reload::installed_exe().map_err(|e| e.to_string());
        let checked = match (&exe, &self.hooks.exec) {
            (Err(e), _) => Err(e.clone()),
            (Ok(_), Some(_)) => Ok(()),
            (Ok(exe), None) => crate::upgrade::can_adopt(exe, &id),
        };
        let mut st = self.lock();
        st.upgrading = false;
        if let Err(e) = checked {
            return self.upgrade_failed(&mut st, &e);
        }
        let exe = exe.unwrap_or_default();
        if !st.upgrade_due || !self.may_move(&st) {
            return; // a turn began meanwhile: the next publish tries again
        }
        let conn = st.conn.clone();
        let agent = match &conn {
            None => None,
            Some(c) => {
                let Some(u) = c.upgrader() else {
                    let e = "its agent can't be handed on while it runs".to_string();
                    return self.upgrade_failed(&mut st, &e);
                };
                match u.pass() {
                    Ok(a) => Some(a),
                    Err(e) => return self.upgrade_failed(&mut st, &e.to_string()),
                }
            }
        };
        let keep = |st: &mut State| {
            if let Some(c) = &conn
                && let Some(u) = c.upgrader()
            {
                u.keep();
            }
            st.upgrade_due = false;
        };
        let k = Kept {
            from: st.info.exe,
            listen: st.listen_fd,
            agent: agent.clone(),
            info: st.info.clone(),
            began: st.began,
            turn: st.turn,
            untold: st.untold.clone(),
            start_cwd: st.start_cwd.clone(),
            ring: st.ring.iter().map(|l| enc(l)).collect(),
            commands: st.commands.as_deref().map(enc).unwrap_or_default(),
            context: st.context.as_deref().map(enc).unwrap_or_default(),
        };
        let path = state_path(&id);
        if let Err(e) = k.write(&path) {
            keep(&mut st);
            return self.upgrade_failed(&mut st, &format!("writing its state: {e}"));
        }
        let mut fds: Vec<RawFd> = vec![st.listen_fd];
        if let Some(a) = &agent {
            fds.extend(a.fds.values().copied());
        }
        let mut kept_open = Ok(());
        for &fd in &fds {
            if let Err(e) = rush_sys::reexec::keep_open(fd) {
                kept_open = Err(format!("keeping file {fd} open: {e}"));
                break;
            }
        }
        let args: Vec<String> = vec![
            exe.to_string_lossy().into_owned(),
            "host".into(),
            "adopt".into(),
            id.clone(),
            path.to_string_lossy().into_owned(),
        ];
        let why = match kept_open {
            Err(e) => e,
            Ok(()) => {
                self.flush_info(&mut st);
                self.event(
                    &st,
                    "host-upgrading",
                    json!({"exe": args[0], "from": st.info.exe, "agentPid": agent.as_ref().map_or(0, |a| a.pid)}),
                );
                crate::events::flush();
                // With the lock held: nothing changes while it goes.
                let e = match &self.hooks.exec {
                    Some(f) => f(&exe, &args),
                    None => rush_sys::reexec::exec(&exe, &args),
                };
                format!("exec {}: {}", args[0], crate::reload::os_err(&e))
            }
        };
        // Still here: it didn't happen. All as it was.
        for &fd in &fds {
            let _ = rush_sys::reexec::close_on_exec(fd);
        }
        let _ = std::fs::remove_file(&path);
        keep(&mut st);
        self.upgrade_failed(&mut st, &why);
    }

    /// The move couldn't be done: the host stays on the rush it runs, its
    /// agent as it was, and says why.
    fn upgrade_failed(&self, st: &mut State, why: &str) {
        st.upgrade_due = false;
        st.info.upgrade = format!("failed: {why}");
        self.event(st, "upgrade-failed", json!({"err": why}));
        self.publish(st);
    }

    /// Takes up the agent the build before passed on, as start would have
    /// started it: the same tools, tap and sink.
    fn adopt_agent(&self, st: &mut State, u: Upgrade) -> Result<(), String> {
        let Some(a) = self.reg.get(&st.cfg.kind) else {
            return Err("rush doesn't know its agent".into());
        };
        let Some(ad) = a.adopter() else {
            return Err(format!("rush can't take {} up as it runs", a.name()));
        };
        let c = st.cfg.clone();
        let mut o = StartOptions {
            profile: Profile {
                kind: a.kind(),
                name: c.account.name.clone(),
                dir: c.account.dir.clone(),
            },
            dir: if st.start_cwd.is_empty() {
                c.cwd.clone()
            } else {
                st.start_cwd.clone()
            },
            session_id: c.session_id.clone(),
            resume: true,
            model: c.model.clone(),
            effort: c.effort.clone(),
            mode: c.permission_mode.clone(),
            lightly: true,
            tools: vec![crate::agtools::tool_server(self.reg.clone(), &c.id)],
            ..StartOptions::default()
        };
        let pid = u.pid;
        let (key, sink) = self.agent_sink(st, &mut o);
        let conn: Arc<dyn Conn> = Arc::from(ad.adopt(o, u, sink).map_err(|e| e.to_string())?);
        st.conn = Some(conn.clone());
        st.conn_key = key;
        st.options.clear();
        st.info.claude_pid = conn.pid().unwrap_or(pid) as i64;
        Ok(())
    }
}

/// `rush host adopt <id> <state>`: what a host execs into to move onto
/// this rush. It takes up where the build before left off.
pub fn adopt(id: &str, state: &Path) -> Result<(), String> {
    rush_events::set_process("host");
    let r = adopt_with(adapter::global().clone(), Hooks::real(), id, state);
    crate::events::flush();
    r
}

/// adopt, with the agents reg knows and hooks: tests take a host up in
/// their own process.
pub fn adopt_with(reg: Registry, hooks: Hooks, id: &str, state: &Path) -> Result<(), String> {
    let k = Kept::take(state)?;
    // The listener is this process's from here, whatever else fails: it
    // closes on the next exec as every other file does.
    let _ = rush_sys::reexec::close_on_exec(k.listen);
    // SAFETY: the listener the build before kept open across the exec,
    // owned by nothing else here.
    let ln = unsafe { UnixListener::from_raw_fd(k.listen) };
    let p = dir(id).join("config.json");
    let b = std::fs::read(&p)
        .map_err(|e| format!("open {}: {}", p.display(), crate::reload::os_err(&e)))?;
    let cfg = Config::parse(&b).map_err(|e| e.to_string())?;
    rush_events::set_cause("");
    let (h, rx) = build(reg, hooks, cfg);
    let sh = h.0.clone();
    {
        let mut st = sh.lock();
        let mut info = k.info.clone();
        info.host_pid = std::process::id() as i64;
        info.exe = st.info.exe;
        info.upgrades = st.info.upgrades;
        info.upgrade.clear();
        info.claude_pid = 0;
        st.info = info;
        st.began = k.began;
        st.turn = k.turn;
        st.untold = k.untold.clone();
        st.start_cwd = k.start_cwd.clone();
        st.ring = k.ring.iter().map(|l| dec(l).into()).collect();
        st.ring_n = st.ring.iter().map(|l| line_len(l)).sum();
        st.commands = (!k.commands.is_empty()).then(|| dec(&k.commands).into());
        st.context = (!k.context.is_empty()).then(|| dec(&k.context).into());
        st.listen_fd = k.listen;
        let away = st.info.away.clone();
        h.set_away(&mut st, away);
        let pid = k.agent.as_ref().map_or(0, |a| a.pid);
        let f = json!({"from": k.from, "exe": st.info.exe, "agentPid": pid});
        log_event(id, "host-upgraded", f);
        if let Some(a) = k.agent.clone()
            && let Err(e) = h.adopt_agent(&mut st, a)
        {
            // Its files closed with the failure: an agent of Claude Code's
            // kind finishes what it writes and ends; the next message
            // starts it again on its conversation.
            h.event(&st, "adopt-failed", json!({"err": e, "agentPid": pid}));
        }
        if st.info.state == "idle" {
            h.arm_idle(&mut st);
        }
        h.publish(&mut st);
    }
    {
        let h2 = h.clone();
        sh.go("agents", move |_| h2.take_agents(rx));
    }
    serve_on(&h, ln, sock_path(id))
}

#[cfg(test)]
mod tests {
    use std::os::fd::IntoRawFd;
    use std::sync::mpsc;
    use std::time::Duration;

    use rush_core::adapter::Kind;

    use super::*;
    use crate::client::Client;
    use crate::lines::Line;
    use crate::server::tests::{in_state, next, registry};
    use crate::testing::Home;

    fn cfg(home: &Home, id: &str) -> Config {
        let hp = home.path().to_string_lossy().into_owned();
        Config {
            id: id.into(),
            session_id: "fake-0000".into(),
            kind: "fake".into(),
            cwd: hp.clone(),
            account: Profile {
                kind: Kind::from("fake"),
                name: "fake".into(),
                dir: hp,
            },
            prompt: "hi".into(),
            ..Config::default()
        }
    }

    fn dial(id: &str) -> Client {
        let w = rush_sys::fswait::Watcher::new();
        w.watch(&[dir(id)]);
        loop {
            if let Ok(c) = Client::dial(id) {
                return c;
            }
            assert!(
                w.wait(None, Some(Duration::from_secs(10))),
                "the host never listened"
            );
        }
    }

    // Asked mid-turn, the host waits for the turn to end; then it passes
    // its agent, its info and the replay on, keeps its socket open, and
    // execs the installed rush. An exec that fails leaves it as it was,
    // its agent kept, saying why.
    #[test]
    fn a_move_waits_for_the_turn_then_passes_everything() {
        let home = Home::new();
        let id = "up1";
        std::fs::create_dir_all(dir(id)).unwrap();
        std::fs::write(dir(id).join("config.json"), cfg(&home, id).to_json_indent()).unwrap();
        let (tx, execs) = mpsc::channel::<(Vec<String>, Kept)>();
        let tx = Mutex::new(tx);
        let mut hooks = Hooks::quiet();
        hooks.exec = Some(Arc::new(move |_exe: &Path, args: &[String]| {
            let b = std::fs::read(&args[4]).unwrap();
            let k: Kept = serde_json::from_slice(&b).unwrap();
            // What it keeps open is open, and kept so.
            // SAFETY: reads the flags of the host's listener.
            let fl = unsafe { libc::fcntl(k.listen, libc::F_GETFD) };
            assert!(
                fl >= 0 && fl & libc::FD_CLOEXEC == 0,
                "the socket closes on exec"
            );
            tx.lock().unwrap().send((args.to_vec(), k)).unwrap();
            std::io::Error::from(std::io::ErrorKind::NotFound)
        }));
        let hp = home.path();
        let server = std::thread::spawn(move || {
            Home::enter(&hp);
            run_with(registry(), hooks, id)
        });
        let c = dial(id);
        let Line::Event(Event::Approval(ap)) =
            next(&c, |l| matches!(l, Line::Event(Event::Approval(_))))
        else {
            unreachable!()
        };
        let Line::Info(i) = next(&c, in_state("blocked")) else {
            unreachable!()
        };
        assert!(i.upgrades, "a fake host moves in place");
        c.send_op(Op::named("upgrade")).unwrap();
        let Line::Info(i) = next(&c, |l| matches!(l, Line::Info(i) if i.upgrade == "due")) else {
            unreachable!()
        };
        assert_eq!(i.state, "blocked");
        assert!(execs.try_recv().is_err(), "moved mid-turn");
        c.allow(&ap.id, None, false).unwrap();
        let (args, k) = execs
            .recv_timeout(Duration::from_secs(10))
            .expect("never moved once the turn ended");
        assert_eq!(&args[1..4], ["host", "adopt", id]);
        assert_eq!(args[4], state_path(id).to_string_lossy());
        assert_eq!(k.info.state, "idle");
        assert_eq!(k.info.session_id, "fake-session");
        assert!(k.began || k.info.turn > 0);
        let a = k.agent.expect("its agent passed on");
        assert_eq!(a.unread, b"half a line");
        let ring: Vec<String> = k
            .ring
            .iter()
            .map(|l| {
                let mut b = Vec::new();
                crate::pack::unpack(&dec(l), &mut b).unwrap();
                String::from_utf8(b).unwrap()
            })
            .collect();
        assert!(ring.iter().any(|l| l.contains("answered yes")), "{ring:?}");
        // Failed, it stays and says so; the state file is gone.
        let Line::Info(i) = next(
            &c,
            |l| matches!(l, Line::Info(i) if i.upgrade.starts_with("failed")),
        ) else {
            unreachable!()
        };
        assert!(i.upgrade.contains("exec"), "{}", i.upgrade);
        assert!(!state_path(id).exists());
        // SAFETY: as above.
        let fl = unsafe { libc::fcntl(k.listen, libc::F_GETFD) };
        assert!(fl & libc::FD_CLOEXEC != 0, "left open for the next exec");
        // Its agent carries on.
        c.send("again").unwrap();
        next(&c, |l| matches!(l, Line::Event(Event::Approval(_))));
        c.stop().unwrap();
        server.join().unwrap().unwrap();
    }

    // The new build takes up where the old left off: the same socket
    // (open across the exec), a client dialling again replayed what the
    // old one had, the agent taken up through its adapter with what it
    // hadn't read, and messages after the move answered.
    #[test]
    fn the_new_build_takes_up_where_the_old_left_off() {
        let home = Home::new();
        let id = "up2";
        std::fs::create_dir_all(dir(id)).unwrap();
        let mut c0 = cfg(&home, id);
        c0.prompt.clear();
        std::fs::write(dir(id).join("config.json"), c0.to_json_indent()).unwrap();
        let sock = sock_path(id);
        let ln = UnixListener::bind(&sock).unwrap();
        // A client dials as the old build execs: it's answered once the
        // new one listens.
        let early = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        let said = crate::lines::event_line(&Event::Status(event::Status {
            busy: false,
            text: "before the move".into(),
        }))
        .unwrap();
        let k = Kept {
            from: BinStamp {
                mod_: rush_state::Time::from_unix(1_700_000_000, 0),
                size: 1,
            },
            listen: ln.into_raw_fd(),
            agent: Some(Upgrade {
                pid: 0,
                unread: b"partial".to_vec(),
                state: json!({"fake": true}),
                ..Upgrade::default()
            }),
            info: Info {
                id: id.into(),
                kind: "fake".into(),
                session_id: "fake-session".into(),
                state: "idle".into(),
                turn: 4,
                generation: 2,
                cost_usd: 0.5,
                ..Info::default()
            },
            began: true,
            turn: 4,
            ring: vec![enc(&crate::pack::pack(&said))],
            ..Kept::default()
        };
        let p = state_path(id);
        k.write(&p).unwrap();
        let hp = home.path();
        let p2 = p.clone();
        let server = std::thread::spawn(move || {
            Home::enter(&hp);
            adopt_with(registry(), Hooks::quiet(), id, &p2)
        });
        drop(early);
        let c = dial(id);
        let mut replay = Vec::new();
        let i = loop {
            let l = c.lines.recv_timeout(Duration::from_secs(10)).unwrap();
            if let Ok(Line::Info(i)) = crate::lines::decode(&l) {
                break i;
            }
            replay.push(String::from_utf8(l).unwrap());
        };
        assert!(
            replay.iter().any(|l| l.contains("before the move")),
            "{replay:?}"
        );
        assert_eq!(i.host_pid, std::process::id() as i64);
        assert_eq!((i.turn, i.generation, i.cost_usd), (4, 2, 0.5));
        assert!(i.upgrade.is_empty() && i.upgrades);
        assert!(!p.exists(), "its state read once");
        let starts = std::fs::read_to_string(home.path().join("starts.log")).unwrap();
        assert_eq!(starts, "adopt partial\n", "taken up, not started");
        c.send("after").unwrap();
        let Line::Event(Event::Approval(ap)) =
            next(&c, |l| matches!(l, Line::Event(Event::Approval(_))))
        else {
            unreachable!()
        };
        assert_eq!(ap.call.input.command, "echo after");
        c.stop().unwrap();
        server.join().unwrap().unwrap();
    }
}
