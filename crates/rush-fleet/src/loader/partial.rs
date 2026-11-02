//! Readings that read again only the sessions whose files can have
//! changed. While an agent works the list is read every second; most of
//! its rows (past conversations, finished jobs, other agents' past
//! sessions) are just as they were. A whole reading keeps them; the
//! readings after make rush's own sessions again and take the rest as
//! the whole reading left them, until something only a whole reading
//! would see changes: a session new or gone, a conversation new or
//! written to that no session has, rush's state, a job, a session open in
//! a terminal, a checkout's branch, a spend that moved, or WHOLE_EVERY
//! passing. What they keep goes through the same last steps as a whole
//! reading's rows (folded spawns, hidden, scratch, pull request,
//! compaction, order), so each is the row a whole reading would make.
//! Rust-only: Go read everything on every tick.

use super::*;

/// How long readings keep the last whole one's other rows: then
/// everything is read again, whatever the watch said.
pub(crate) const WHOLE_EVERY: Ms = MINUTE;

/// What a whole reading leaves for the readings after it.
#[derive(Default)]
pub(crate) struct Whole {
    pub(super) at: Ms,
    /// It allows readings that keep its rows: none of them can change
    /// without a file the watch tells of changing.
    pub(super) ok: bool,
    pub(super) skip_past: bool,
    pub(super) priced: bool,
    pub(super) profile: Option<Profile>,
    /// The home account as counted before rush's sessions were.
    pub(super) av: Option<AccountView>,
    /// The rows before rush's sessions (jobs, terminal sessions), the
    /// past conversations, and other agents' sessions.
    pub(super) before: Vec<Agent>,
    pub(super) past: Vec<Agent>,
    pub(super) others: Vec<Agent>,
    /// Where those were in the reading.
    pub(super) at_hosted: usize,
    pub(super) at_past: usize,
    pub(super) at_after: usize,
    /// Their keys.
    pub(super) kept: HashSet<String>,
    /// rush's sessions.
    pub(super) hosted_ids: HashSet<String>,
    /// Those stopped or asleep, by id, with their info, as last made.
    /// Boxed, as Loader's idle_rows: an entry is over two kilobytes, and
    /// the table, a thousand sessions and more, keeps room for as many
    /// again (see [`IdleEntry`]).
    pub(super) idle: HashMap<String, IdleEntry>,
    /// The keys of those the last reading took as they were.
    pub(super) reused: HashSet<String>,
    /// The conversations rush's sessions stand for (and left behind).
    pub(super) claims: HashSet<String>,
    /// Every conversation a row stood for.
    pub(super) claimed: HashSet<String>,
    /// Each adapter's live sessions, as read.
    pub(super) live: Vec<Vec<LiveSig>>,
    /// The home profile's past listing, by number.
    pub(super) past_ver: Option<u64>,
    /// The processes of the sessions open in a terminal.
    pub(super) sess_pids: Vec<Pid>,
    /// Headless runs those sessions' shells ran.
    pub(super) spawned: Vec<(String, Pid)>,
}

/// A rush session asleep or stopped, its info and its row, as a whole
/// reading's `idle` keeps it.
pub(crate) type IdleEntry = Box<(HostInfo, Agent)>;

/// What a live session said of itself: one differing makes its row
/// differ.
#[derive(Debug, PartialEq)]
pub(crate) struct LiveSig {
    id: String,
    name: String,
    state: String,
    detail: String,
    needs: String,
    cwd: String,
    model: String,
    transcript: String,
    pid: i32,
    headless: bool,
    interactive: bool,
    remote: bool,
    created_at: Ms,
    updated_at: Ms,
    status: String,
    status_at: Ms,
    job_id: String,
    repo: String,
    prs: Vec<rush_core::adapter::Pr>,
    todos: Vec<rush_core::agent::Todo>,
    running: Vec<Task>,
    job: Option<Job>,
}

pub(crate) fn sigs(live: &[Session]) -> Vec<LiveSig> {
    live.iter()
        .map(|s| LiveSig {
            id: s.id.clone(),
            name: s.name.clone(),
            state: s.state.clone(),
            detail: s.detail.clone(),
            needs: s.needs.clone(),
            cwd: s.cwd.clone(),
            model: s.model.clone(),
            transcript: s.transcript.clone(),
            pid: s.pid,
            headless: s.headless,
            interactive: s.interactive,
            remote: s.remote,
            created_at: s.created_at,
            updated_at: s.updated_at,
            status: s.status.clone(),
            status_at: s.status_at,
            job_id: s.job_id.clone(),
            repo: s.repo.clone(),
            prs: s.prs.clone(),
            todos: s.todos.clone(),
            running: s.running.clone(),
            job: s.job.as_deref().cloned(),
        })
        .collect()
}

/// A rush session nothing runs for: its row changes only with its info.
pub(crate) fn idle(i: &HostInfo) -> bool {
    i.state == "stopped" || i.sleeping
}

/// A row a reading can keep: nothing of it runs, so only its files
/// changing, which the watch tells, changes it.
pub(crate) fn keeps(a: &Agent) -> bool {
    !a.rush
        && a.pid == 0
        && !a.job.live()
        && !a.busy()
        && a.job.in_flight == 0
        && a.subs.direct + a.subs.nested == 0
}

impl Loader {
    /// Has readings after a whole one read again only rush's sessions
    /// while nothing else changed, keeping every other row as it was,
    /// with a whole reading at least every WHOLE_EVERY. It needs a shared
    /// watch (watch_shared) to know what changed.
    pub fn incremental(&mut self, on: bool) {
        self.incremental = on;
        if !on {
            self.whole = None;
        }
    }

    /// Whether the next reading reads only rush's sessions, as far as
    /// what the watch has told so far says (the reading itself may still
    /// find it must read everything): the rows it keeps (`kept`) needn't
    /// have their transcripts priced before it.
    pub fn keeping(&self, now: Ms) -> bool {
        let (Some(w), Some(wt)) = (self.whole.as_deref(), self.watching.as_ref()) else {
            return false;
        };
        self.incremental
            && wt.shared
            && !self.quick
            && !wt.told_all
            && self.allows(w, now)
            && wt.told.iter().all(|p| local(p, &w.hosted_ids))
    }

    /// Whether the last reading took row key as an earlier one made it:
    /// one of the whole reading's rows left for the readings after it, or
    /// a rush session asleep or stopped since. Its transcript can't have
    /// grown.
    pub fn kept(&self, key: &str) -> bool {
        self.whole
            .as_deref()
            .is_some_and(|w| w.ok && (w.kept.contains(key) || w.reused.contains(key)))
    }

    /// What allows w's rows to be kept, without reading anything.
    fn allows(&self, w: &Whole, now: Ms) -> bool {
        w.ok && now - w.at < WHOLE_EVERY
            && w.skip_past == self.handle.skip_past.load(Ordering::Relaxed)
            && w.priced == self.priced
            && (w.skip_past || self.others.values().all(|(at, _)| now - at < OTHERS_EVERY))
            && !self.spend_dirty.iter().any(|k| w.kept.contains(k))
    }

    /// A reading that makes rush's sessions again and keeps w's other
    /// rows; None when something only a whole reading would see changed.
    pub(super) fn load_part(&mut self, now: Ms, mut w: Box<Whole>) -> Option<Snapshot> {
        let wt = self.watching.as_ref()?;
        if !self.incremental
            || !wt.shared
            || wt.all
            || !self.allows(&w, now)
            || !wt.chg.iter().all(|p| local(p, &w.hosted_ids))
            || crate::git::git_moved(&mut self.git, now)
        {
            return None;
        }
        // The sessions whose folders changed: made again, however idle.
        let told: HashSet<String> = wt
            .chg
            .iter()
            .filter_map(|p| p.strip_prefix(host_root()).ok()?.components().next())
            .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect();
        self.began(now);
        let checked = self.checked();
        let hosted = {
            let w = self.watching.as_ref();
            let quick = self.quick;
            let fresh = |p: &Path, at: u64| crate::watch::fresh(w, quick, p, at);
            self.hosts.list(&fresh, checked)
        };
        if hosted.iter().any(|i| i.lost)
            || hosted.len() != w.hosted_ids.len()
            || hosted.iter().any(|i| !w.hosted_ids.contains(&i.id))
        {
            return None;
        }
        let mut claims: HashSet<String> = hosted.iter().map(|i| i.session_id.clone()).collect();
        self.branches(&hosted, &mut claims);
        if claims != w.claims {
            return None;
        }
        self.sync_usage();
        let (ad, p, _) = self.home_profile()?;
        if w.profile.as_ref() != Some(&p) {
            return None;
        }
        if self.live_now(&ad, &p) != w.live {
            return None;
        }
        if !w.skip_past {
            match w.past_ver {
                Some(v) => match ad.discoverer().and_then(|d| d.past_changes(&p, v)) {
                    // Only the conversations rows stand for were written
                    // to: no past row is any different.
                    Some((nv, Some(ids))) if ids.iter().all(|id| w.claimed.contains(id)) => {
                        w.past_ver = Some(nv);
                        self.whole_past = Some((p.dir.clone(), nv));
                    }
                    _ => return None,
                },
                None if now - w.at >= PAST_EVERY => return None,
                None => {}
            }
        }
        let mut tab = Some(self.procs.snapshot());
        self.pending_subs = Some(Vec::new());
        let mut av = w.av.clone()?;
        av.usage = self.read_usage(&ad, &p, now);
        av.quota = av.usage.quota(&reading_key(&p.dir, &av.usage));
        let mut agents = Vec::with_capacity(w.kept.len() + hosted.len());
        agents.extend(w.before.iter().cloned());
        w.reused.clear();
        let mut made = Vec::new();
        for info in hosted.iter().filter(|i| i.kind == p.kind.as_str()) {
            let a = self.hosted_row(
                &mut w,
                &told,
                &p,
                info,
                tab.as_mut(),
                now,
                &mut made,
                agents.len(),
            );
            count(&mut av, &a);
            agents.push(a);
        }
        for a in &w.past {
            av.agents += 1;
            av.spend += a.spend.cost;
            av.today += a.spend.today;
        }
        agents.extend(w.past.iter().cloned());
        for info in hosted.iter().filter(|i| i.kind != p.kind.as_str()) {
            let other = Profile {
                kind: p.kind.clone(),
                name: info.account.clone(),
                dir: String::new(),
            };
            let a = self.hosted_row(
                &mut w,
                &told,
                &other,
                info,
                tab.as_mut(),
                now,
                &mut made,
                agents.len(),
            );
            agents.push(a);
        }
        agents.extend(w.others.iter().cloned());
        self.finish_subs(&mut agents);
        for (id, info, i) in made {
            w.idle.insert(id, Box::new((info, agents[i].clone())));
        }
        let mut parents: HashSet<Pid> = HashSet::new();
        for i in hosted.iter() {
            parents.insert(i.claude_pid);
            parents.insert(i.host_pid);
        }
        parents.extend(w.sess_pids.iter().copied());
        let snap = Snapshot {
            at: now,
            agents,
            accounts: vec![av],
            ..Snapshot::default()
        };
        let spawned = w.spawned.clone();
        let mut snap = self.tail(snap, tab, hosted, spawned, parents, now, true);
        snap.partial = true;
        self.spend_dirty.clear();
        self.whole = Some(w);
        Some(snap)
    }

    /// A rush session's row: as the last reading made it when it's asleep
    /// or stopped and nothing of it changed since, else made again (and
    /// noted in made, to be kept once its subagents are counted, at i).
    #[allow(clippy::too_many_arguments)]
    fn hosted_row(
        &mut self,
        w: &mut Whole,
        told: &HashSet<String>,
        p: &Profile,
        info: &HostInfo,
        tab: Option<&mut Table>,
        now: Ms,
        made: &mut Vec<(String, HostInfo, usize)>,
        i: usize,
    ) -> Agent {
        if idle(info)
            && !told.contains(&info.id)
            && let Some((was, a)) = w.idle.get(&info.id).map(|e| &**e)
            && was == info
            && !self.spend_dirty.contains(&a.key)
        {
            w.reused.insert(a.key.clone());
            return a.clone();
        }
        let a = self.hosted_agent(p, info, tab, now);
        if idle(info) {
            made.push((info.id.clone(), info.clone(), i));
        } else {
            w.idle.remove(&info.id);
        }
        a
    }

    /// Each adapter's live sessions, read as a whole reading reads them:
    /// the home profile's, then every other agent's profiles'.
    fn live_now(&self, home: &AdapterRef, p: &Profile) -> Vec<Vec<LiveSig>> {
        let mut out = vec![sigs(
            &home.discoverer().map(|d| d.live(p)).unwrap_or_default(),
        )];
        for a in self.reg.all().iter() {
            let Some(d) = a.discoverer() else { continue };
            if a.kind() == p.kind {
                continue;
            }
            for pp in a.profiles() {
                out.push(sigs(&d.live(&pp)));
            }
        }
        out
    }
}

/// Whether a change at path is one of rush's sessions' own (its folder,
/// or anything in it): reading those sessions again takes it in.
fn local(path: &Path, ids: &HashSet<String>) -> bool {
    let root = host_root();
    let Ok(rest) = path.strip_prefix(&root) else {
        return false;
    };
    rest.components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|id| ids.contains(id))
}
