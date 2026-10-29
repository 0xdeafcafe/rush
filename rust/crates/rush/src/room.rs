//! Rooms' sessions through rush's host module: what rush-room's driver and
//! `rush room new` ask of it (rush_room::Sessions, rush_room::MemberConfig),
//! answered by rush-host.
//!
//! Ported from internal/room/run.go:110-150 (open: host.Spawn,
//! host.LogEvent, host.Dial, host.Decode with the native agent's Neutral),
//! run.go:489-510 (host.InfoEvent, host.FailReport, host.ErrorEvent),
//! run.go:567-582 (Allow, Deny), room.go:138 (host.NewSessionID) and
//! cmd/rush/room.go:128-146 (roomMemberConfig).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use rush_core::adapter::{Kind, Profile, global};
use rush_host::lines::{Decoder, Line};
use rush_host::{Client, Config, Info};
use rush_room::{
    MemberConfig, MemberEvent, Msg, RetryInfo, RoomCtx, Rooms, Session, SessionInfo, Sessions,
};
use rush_state::Start;
use rush_state::json::RawJson;
use serde_json::{Map, Value};

/// What `rush room` works with in this rush: its rooms under rush's
/// folder, the agents rush knows, and sessions run by rush's host.
pub fn ctx() -> RoomCtx {
    RoomCtx {
        rooms: Rooms::new(),
        agents: Arc::new(rush_room::Registered),
        sessions: Some(Arc::new(HostSessions)),
        configs: Some(Arc::new(HostConfigs)),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rush")),
        cwd: std::env::current_dir().unwrap_or_default(),
    }
}

/// A room's members' sessions, each its own host (host.Spawn, host.Dial).
struct HostSessions;

impl Sessions for HostSessions {
    fn spawn(&self, cfg: Value) -> Result<String, String> {
        let b = serde_json::to_vec(&cfg).map_err(|e| e.to_string())?;
        // parse, as config.json is read: the account's kind is the session's
        let cfg = Config::parse(&b).map_err(|e| e.to_string())?;
        rush_host::spawn(cfg)
            .map(|c| c.id)
            .map_err(|e| e.to_string())
    }

    fn dial(&self, id: &str, i: usize, tx: SyncSender<Msg>) -> Result<Box<dyn Session>, String> {
        let mut c = Client::dial(id).map_err(|e| e.to_string())?;
        let (_, none) = sync_channel(0);
        let lines = std::mem::replace(&mut c.lines, none);
        std::thread::Builder::new()
            .name("room-member".into())
            .spawn(move || follow(lines, i, &tx))
            .map_err(|e| e.to_string())?;
        Ok(Box::new(HostSession(c)))
    }

    fn log_event(&self, id: &str, kind: &str, fields: Map<String, Value>) {
        rush_host::events::log_event(id, kind, Value::Object(fields));
    }

    fn fail_report(&self, kind: &str, err: &str, step: &str, tries: i64) -> String {
        rush_host::subtool::fail_report(global(), kind, err, step, tries)
    }
}

/// Hands member i's lines to the driver as its events, a batch a line,
/// and a last closed batch once they end (run.go:130-148). One Decoder
/// follows the connection: the native agent's own lines come out as
/// rush's events, as Go's Neutral makes them.
fn follow(lines: Receiver<Vec<u8>>, i: usize, tx: &SyncSender<Msg>) {
    let mut d = Decoder::default();
    for l in lines {
        let Ok(ls) = d.decode(&l) else { continue };
        let evs: Vec<MemberEvent> = ls.into_iter().filter_map(member_event).collect();
        if evs.is_empty() {
            continue; // the host's own traffic: nothing the driver reads
        }
        let m = Msg::Member {
            i,
            evs,
            closed: false,
        };
        if tx.send(m).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::Member {
        i,
        evs: Vec::new(),
        closed: true,
    });
}

/// What one of a host's lines is to the driver: rush's events, the
/// session's info and the host's errors; nothing for the rest (answered,
/// echoes, stamps, commands, replies, context, a native line no agent
/// reads), which Go's driver passes to answer and it ignores.
fn member_event(l: Line) -> Option<MemberEvent> {
    match l {
        Line::Event(e) => Some(MemberEvent::Event(Box::new(e))),
        Line::Info(i) => Some(MemberEvent::Info(session_info(&i))),
        Line::Error(e) => Some(MemberEvent::Error(e)),
        _ => None,
    }
}

/// A session's info, as far as the driver reads it (run.go:489-506).
fn session_info(i: &Info) -> SessionInfo {
    SessionInfo {
        kind: i.kind.clone(),
        model: i.model.clone(),
        state: i.state.clone(),
        retry: i.retry.as_ref().map(|r| RetryInfo {
            gave_up: r.gave_up,
            last_step: r.last_step.clone(),
            attempt: r.attempt,
        }),
        background: i.background.len(),
        queue: i.queue.len(),
        limited: i.limit.is_some(),
    }
}

/// One member's session, dialled.
struct HostSession(Client);

impl Session for HostSession {
    fn send(&self, text: &str) -> Result<(), String> {
        self.0.send(text).map_err(|e| e.to_string())
    }
    fn send_guide(&self, text: &str) -> Result<(), String> {
        self.0.send_guide(text, &[]).map_err(|e| e.to_string())
    }
    fn interrupt(&self) -> Result<(), String> {
        self.0.interrupt().map_err(|e| e.to_string())
    }
    fn allow(&self, id: &str) -> Result<(), String> {
        self.0.allow(id, None, false).map_err(|e| e.to_string())
    }
    fn deny(&self, id: &str, why: &str) -> Result<(), String> {
        self.0.deny(id, why, false).map_err(|e| e.to_string())
    }
    fn stop(&self) -> Result<(), String> {
        self.0.stop().map_err(|e| e.to_string())
    }
    fn close(&self) {
        self.0.close();
    }
}

/// New members' configs, as Settings starts a fresh session.
struct HostConfigs;

impl MemberConfig for HostConfigs {
    /// cmd/rush/room.go:130 (roomMemberConfig).
    fn member_config(
        &self,
        kind: &str,
        model: &str,
        effort: &str,
        cwd: &str,
    ) -> Result<RawJson, String> {
        let reg = global();
        let st = rush_state::Store::load();
        let d = &st.config.dispatch;
        let mut cfg = Config {
            cwd: cwd.to_string(),
            ..Config::default()
        };
        if kind == rush_state::LOGINS_KIND {
            let f = st.config.active_account();
            cfg.account = Profile {
                kind: Kind::from(rush_state::LOGINS_KIND),
                name: f.name,
                dir: f.config_dir,
            };
        }
        cfg.use_agent(reg, kind)?;
        let _ = rush_host::kind::sign_in_if_out(reg, kind, &cfg.account);
        fresh(
            &mut cfg,
            &d.resolved_for(kind),
            d.quick_start(),
            model,
            effort,
        );
        RawJson::from_value(&cfg).map_err(|e| e.to_string())
    }

    fn new_session_id(&self) -> String {
        rush_host::new_session_id().1
    }
}

/// Starts cfg as Settings does: lean or not, on model and effort when
/// given, else start's, in start's permission mode.
fn fresh(cfg: &mut Config, start: &Start, lean: bool, model: &str, effort: &str) {
    let or = |a: &str, b: &str| if a.is_empty() { b } else { a }.to_string();
    cfg.lean = lean;
    cfg.model = or(model, &start.model);
    cfg.effort = or(effort, &start.effort);
    cfg.permission_mode = start.mode.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::event::{Event, TurnEnd};
    use rush_host::lines::{error_line, event_line, info_line};
    use rush_host::{Limit, Retry, Task};

    /// A config as roomMemberConfig makes it is written as Go writes it
    /// (Go's jsonx.Marshal of host.Config, run against internal/host).
    #[test]
    fn member_config_json_is_gos() {
        let cases = [
            (
                Config {
                    cwd: "/work/<x>".into(),
                    kind: "claude".into(),
                    account: Profile {
                        kind: Kind::from("claude"),
                        name: "Work".into(),
                        dir: "/home/.claude".into(),
                    },
                    ..Config::default()
                },
                Start {
                    model: "sonnet".into(),
                    effort: "low".into(),
                    mode: "acceptEdits".into(),
                },
                true,
                "opus",
                "high",
                r#"{"id":"","sessionId":"","resume":false,"cwd":"/work/<x>","model":"opus","effort":"high","permissionMode":"acceptEdits","lean":true,"kind":"claude","account":{"name":"Work","configDir":"/home/.claude"}}"#,
            ),
            (
                Config {
                    cwd: "/w".into(),
                    kind: "codex".into(),
                    account: Profile {
                        kind: Kind::from("codex"),
                        name: String::new(),
                        dir: "/home/.codex".into(),
                    },
                    ..Config::default()
                },
                Start::default(),
                false,
                "",
                "",
                r#"{"id":"","sessionId":"","resume":false,"cwd":"/w","kind":"codex","account":{"name":"","configDir":"/home/.codex"}}"#,
            ),
        ];
        for (mut cfg, start, lean, model, effort, want) in cases {
            fresh(&mut cfg, &start, lean, model, effort);
            assert_eq!(RawJson::from_value(&cfg).unwrap().get(), want);
        }
    }

    /// Start's model and effort stand in for those not given.
    #[test]
    fn fresh_falls_back_to_start() {
        let mut cfg = Config::default();
        let start = Start {
            model: "sonnet".into(),
            effort: "low".into(),
            mode: "plan".into(),
        };
        fresh(&mut cfg, &start, false, "", "max");
        assert_eq!(
            (
                cfg.model.as_str(),
                cfg.effort.as_str(),
                cfg.permission_mode.as_str()
            ),
            ("sonnet", "max", "plan")
        );
    }

    /// The config the driver hands spawn reads back as the host takes it:
    /// the account's kind is the session's.
    #[test]
    fn spawn_config_reads_back() {
        let v: Value = serde_json::from_str(
            r#"{"id":"","sessionId":"","resume":false,"cwd":"/w","name":"Room · A · t","meta":{"room":"r1","roomMember":"A"},"kind":"codex","systemPrompt":"s","owner":42,"account":{"name":"","configDir":"/h"}}"#,
        )
        .unwrap();
        let cfg = Config::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(cfg.owner, 42);
        assert_eq!(cfg.account.kind.as_str(), "codex");
        assert_eq!(cfg.account.dir, "/h");
        assert_eq!(cfg.meta.get("roomMember").map(String::as_str), Some("A"));
        assert_eq!(cfg.system_prompt, "s");
    }

    fn info(i: &Info) -> Vec<u8> {
        info_line(&serde_json::to_vec(i).unwrap())
    }

    /// A host's lines reach the driver as its events, a batch a line; the
    /// host's own traffic doesn't, and a last batch says they've ended.
    #[test]
    fn host_lines_become_member_events() {
        let turn_end = Event::TurnEnd(TurnEnd {
            reason: "error".into(),
            err: "overloaded".into(),
            ..TurnEnd::default()
        });
        let full = Info {
            kind: "codex".into(),
            model: "gpt-5".into(),
            state: "idle".into(),
            retry: Some(Retry {
                gave_up: true,
                last_step: "Bash (ls)".into(),
                attempt: 3,
                ..Retry::default()
            }),
            background: vec![Task::default(), Task::default()],
            queue: vec!["later".into()],
            limit: Some(Limit::default()),
            ..Info::default()
        };
        let (ltx, lrx) = sync_channel(16);
        for l in [
            event_line(&turn_end).unwrap(),
            info(&full),
            info(&Info {
                state: "working".into(),
                ..Info::default()
            }),
            error_line("no such request"),
            br#"{"request_id":"r1","type":"agtop_answered"}"#.to_vec(),
            b"not json".to_vec(),
        ] {
            ltx.send(l).unwrap();
        }
        drop(ltx);
        let (tx, rx) = sync_channel(16);
        follow(lrx, 2, &tx);
        drop(tx);
        let got: Vec<(usize, Vec<MemberEvent>, bool)> = rx
            .into_iter()
            .map(|m| match m {
                Msg::Member { i, evs, closed } => (i, evs, closed),
                Msg::User(_) => panic!("a user's entry from a session"),
            })
            .collect();
        let want = vec![
            (2, vec![MemberEvent::Event(Box::new(turn_end))], false),
            (
                2,
                vec![MemberEvent::Info(SessionInfo {
                    kind: "codex".into(),
                    model: "gpt-5".into(),
                    state: "idle".into(),
                    retry: Some(RetryInfo {
                        gave_up: true,
                        last_step: "Bash (ls)".into(),
                        attempt: 3,
                    }),
                    background: 2,
                    queue: 1,
                    limited: true,
                })],
                false,
            ),
            (
                2,
                vec![MemberEvent::Info(SessionInfo {
                    kind: "claude".into(), // migrated: an older host wrote none
                    state: "working".into(),
                    ..SessionInfo::default()
                })],
                false,
            ),
            (2, vec![MemberEvent::Error("no such request".into())], false),
            (2, vec![], true),
        ];
        assert_eq!(got, want);
    }
}
