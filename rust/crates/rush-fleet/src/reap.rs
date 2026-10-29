//! What an agent leaves running when it stops: shells, servers and
//! watchers its tools started, which the system hands to launchd (pid 1)
//! once the agent's process is gone and which would otherwise run for
//! days. Only a process seen under an agent is ever named, by its pid and
//! start time, so nothing else is touched. Go: internal/fleet/reap.go
//! (Reaper :18, Leftover :36, Describe :56, Scan :60, ownerOf :144, alive
//! :160). It only reads the table it's given; ending one is
//! rush_sys::actions::end_tree, off the UI.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use rush_sys::proc::{Pid, Proc, Table};

/// What the reaper saw under an agent.
#[derive(Clone, Debug)]
struct Seen {
    start: SystemTime,
    /// The agent's name, for saying what was ended.
    agent: String,
    /// The agent's program it ran under: only once that has exited is it
    /// left behind. Something the agent set running on its own (nohup, a
    /// daemon) is left alone while the agent works.
    owner: Pid,
    owner_start: SystemTime,
}

/// Notes the processes under every agent, and names those that have since
/// been left behind.
#[derive(Clone, Debug, Default)]
pub struct Reaper {
    seen: HashMap<Pid, Seen>,
}

/// A process an agent left behind, with everything under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub pid: Pid,
    pub start: SystemTime,
    pub agent: String,
    /// Its command line: empty until described.
    pub cmd: String,
    pub procs: usize,
}

impl Leftover {
    /// Fills in its command line, which asks the system.
    pub fn describe(&mut self) {
        self.cmd = rush_sys::proc::command_line(self.pid);
    }
}

impl Reaper {
    /// Notes what runs under each agent (pid, name) in tab, and returns
    /// what was noted before and has since been orphaned: alive, the same
    /// process, no longer under any agent, adopted by launchd, its owner
    /// gone. me is rush's own pid (never an agent here); is_program says
    /// whether a command is an agent's program.
    pub fn scan(
        &mut self,
        tab: &Table,
        agents: &[(Pid, String)],
        me: Pid,
        is_program: &dyn Fn(&str) -> bool,
    ) -> Vec<Leftover> {
        let mut under = HashSet::new();
        for (apid, name) in agents {
            if *apid == 0 || *apid == me {
                continue;
            }
            let Some(root) = tab.procs.get(apid) else {
                continue;
            };
            for pid in tab.descendants(*apid) {
                under.insert(pid);
                if pid == *apid {
                    continue;
                }
                let Some(p) = tab.procs.get(&pid) else {
                    continue;
                };
                if self.seen.get(&pid).is_some_and(|s| s.start == p.start) {
                    continue;
                }
                let Some(owner) = owner_of(tab, root, p, is_program) else {
                    continue;
                };
                self.seen.insert(
                    pid,
                    Seen {
                        start: p.start,
                        agent: name.clone(),
                        owner: owner.pid,
                        owner_start: owner.start,
                    },
                );
            }
        }
        let mut out = Vec::new();
        let mut pids: Vec<Pid> = self.seen.keys().copied().collect();
        pids.sort_unstable();
        for pid in pids {
            let s = &self.seen[&pid];
            match tab.procs.get(&pid) {
                Some(p) if p.start == s.start => {
                    if under.contains(&pid) {
                        continue;
                    }
                    if p.ppid == 1 && !alive(tab, s.owner, s.owner_start) {
                        // Adopted by launchd, so the top of what was left:
                        // ending it ends what's under it too.
                        out.push(Leftover {
                            pid,
                            start: s.start,
                            agent: s.agent.clone(),
                            cmd: String::new(),
                            procs: tab.descendants(pid).len(),
                        });
                        self.seen.remove(&pid);
                    }
                }
                _ => {
                    self.seen.remove(&pid); // gone, or the pid was reused
                }
            }
        }
        out
    }
}

/// The agent's process p runs under, for an agent whose process is root:
/// root itself when it's an agent's program, else (a rush host) the child
/// of root p descends from. p is never its own owner.
fn owner_of<'a>(
    tab: &'a Table,
    root: &'a Proc,
    p: &'a Proc,
    is_program: &dyn Fn(&str) -> bool,
) -> Option<&'a Proc> {
    if is_program(&root.comm) {
        return Some(root);
    }
    let mut q = Some(p);
    for _ in 0..64 {
        let cur = q?;
        if cur.ppid == root.pid {
            return (cur.pid != p.pid).then_some(cur);
        }
        q = tab.procs.get(&cur.ppid);
    }
    None
}

fn alive(tab: &Table, pid: Pid, start: SystemTime) -> bool {
    tab.procs.get(&pid).is_some_and(|p| p.start == start)
}

/// The command a Bash-tool shell was running, without Claude Code's
/// wrapper around it (cleanup.go:600 orphanWhat).
pub fn orphan_what(cmd: &str) -> String {
    let Some(i) = cmd.find("eval '") else {
        return cmd.to_string();
    };
    let mut rest = cmd[i + 6..].replace(r#"'"'"'"#, "'");
    if let Some(j) = rest.find("' ") {
        rest.truncate(j);
    }
    rest.strip_suffix('\'').unwrap_or(&rest).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    fn table(procs: &[(Pid, Pid, &str, u64)]) -> Table {
        let mut t = Table {
            at: std::time::Instant::now(),
            procs: Default::default(),
            children: Default::default(),
        };
        for &(pid, ppid, comm, start) in procs {
            t.procs.insert(
                pid,
                Proc {
                    pid,
                    ppid,
                    comm: Arc::from(comm),
                    start: at(start),
                    ..Proc::default()
                },
            );
            t.children.entry(ppid).or_default().push(pid);
        }
        t
    }

    fn claude(c: &str) -> bool {
        c.rsplit('/').next() == Some("claude")
    }

    #[test]
    fn a_server_left_by_an_exited_agent_is_named_once() {
        let mut r = Reaper::default();
        let agents = vec![(10, "fix the tests".to_string())];
        // claude 10 runs zsh 11, which runs a dev server 12.
        let t = table(&[
            (10, 5, "claude", 100),
            (11, 10, "zsh", 101),
            (12, 11, "node", 102),
        ]);
        assert!(r.scan(&t, &agents, 1, &claude).is_empty());
        // claude exits: the shell is handed to launchd with the server under it.
        let t = table(&[(11, 1, "zsh", 101), (12, 11, "node", 102)]);
        let got = r.scan(&t, &agents, 1, &claude);
        assert_eq!(
            got,
            vec![Leftover {
                pid: 11,
                start: at(101),
                agent: "fix the tests".into(),
                cmd: String::new(),
                procs: 2,
            }]
        );
        assert!(r.scan(&t, &agents, 1, &claude).is_empty(), "once");
    }

    #[test]
    fn what_the_agent_still_runs_or_a_reused_pid_stays() {
        let mut r = Reaper::default();
        let agents = vec![(10, "a".to_string())];
        let t = table(&[(10, 5, "claude", 100), (11, 10, "node", 101)]);
        r.scan(&t, &agents, 1, &claude);
        // Daemonised (adopted by launchd) while its agent still runs.
        let t = table(&[(10, 5, "claude", 100), (11, 1, "node", 101)]);
        assert!(r.scan(&t, &agents, 1, &claude).is_empty());
        // The pid comes back as another process.
        let t = table(&[(11, 1, "node", 999)]);
        assert!(r.scan(&t, &agents, 1, &claude).is_empty());
    }

    #[test]
    fn under_a_host_the_owner_is_the_hosts_child() {
        let mut r = Reaper::default();
        // A rush host 20 runs claude 21, which runs a watcher 22; the
        // host's own child is never a leftover.
        let agents = vec![(20, "hosted".to_string())];
        let t = table(&[
            (20, 1, "rush", 100),
            (21, 20, "claude", 101),
            (22, 21, "node", 102),
        ]);
        r.scan(&t, &agents, 1, &claude);
        // claude restarts under the host; the watcher was handed to launchd.
        let t = table(&[
            (20, 1, "rush", 100),
            (23, 20, "claude", 200),
            (22, 1, "node", 102),
        ]);
        let got = r.scan(&t, &agents, 1, &claude);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pid, 22);
    }

    #[test]
    fn orphan_what_unwraps_the_shell() {
        for (cmd, want) in [
            ("node server.js", "node server.js"),
            (
                "/bin/zsh -c source ~/.claude/shell-snapshots/x.sh && eval 'pnpm dev' \\< /dev/null && pwd -P >| /tmp/c",
                "pnpm dev",
            ),
            ("zsh -c eval 'echo '\"'\"'hi'\"'\"''", "echo 'hi'"),
        ] {
            assert_eq!(orphan_what(cmd), want, "{cmd}");
        }
    }
}
