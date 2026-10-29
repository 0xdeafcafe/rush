//! An agent's own screens with the terminal handed over: a background
//! session joined full screen, the agent's own attach when joining fails,
//! a session its agent let go moved to rush mode, and Claude Code's own
//! screens; after Go's model.go:1327-1353, :2663-2687 and
//! sessionviews.go:1097-1137. A test has no terminal: what's handed it
//! runs at Model::run_outside, never within the key that asked.

use std::process::Command;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, Adapter, Attacher, Elsewhere, Features, Gone, Joiner, Kind, Level, Mirror, Profile,
    Registry, Screener, Terminal,
};
use rush_core::agent::{Agent, Job, State};

use crate::frame::Out;
use crate::model::Model;
use crate::screens::session_core::host::{Fake, set_hosts};

/// How the fake's joined screen ends.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ends {
    Left,
    Gone,
    Elsewhere,
    Broke,
}

#[derive(Clone)]
struct Jobs {
    ends: Arc<Mutex<Ends>>,
    /// The sessions joined, attached and the screens opened, in order.
    did: Arc<Mutex<Vec<String>>>,
    /// The program attach and screen hand back.
    prog: Arc<Mutex<String>>,
}

impl Jobs {
    fn new() -> Jobs {
        Jobs {
            ends: Arc::new(Mutex::new(Ends::Left)),
            did: Arc::default(),
            prog: Arc::new(Mutex::new("/usr/bin/true".into())),
        }
    }
    fn did(&self) -> Vec<String> {
        self.did.lock().unwrap().clone()
    }
}

struct Screen {
    id: String,
    jobs: Jobs,
}

impl Terminal for Screen {
    fn run(&mut self) -> adapter::Result<()> {
        self.jobs
            .did
            .lock()
            .unwrap()
            .push(format!("ran {}", self.id));
        match *self.jobs.ends.lock().unwrap() {
            Ends::Left => Ok(()),
            Ends::Gone => Err(Box::new(Gone)),
            Ends::Elsewhere => Err(Box::new(Elsewhere)),
            Ends::Broke => Err("the socket broke".into()),
        }
    }
    fn set_stdin(&mut self, _: Box<dyn std::io::Read + Send>) {}
    fn set_stdout(&mut self, _: Box<dyn std::io::Write + Send>) {}
    fn set_stderr(&mut self, _: Box<dyn std::io::Write + Send>) {}
}

impl Adapter for Jobs {
    fn kind(&self) -> Kind {
        Kind::from("jobs")
    }
    fn name(&self) -> String {
        "Jobs Code".into()
    }
    fn features(&self) -> Features {
        // It picks a conversation up again, as moving one to rush mode
        // needs.
        [(
            rush_core::adapter::Feature::Resume,
            rush_core::adapter::Support::YES,
        )]
        .into_iter()
        .collect()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(joiner: Joiner, attacher: Attacher, screener: Screener);
}

impl Joiner for Jobs {
    fn join(&self, p: &Profile, id: &str) -> Box<dyn Terminal> {
        self.did
            .lock()
            .unwrap()
            .push(format!("join {id} as {}", p.name));
        Box::new(Screen {
            id: id.into(),
            jobs: self.clone(),
        })
    }
    fn service_up(&self, _: &Profile) -> bool {
        true
    }
    fn mirror(&self, _: &Profile, _: &str, _: &str, _: u16, _: u16) -> adapter::Result<Mirror> {
        Err("no mirror".into())
    }
    fn resize(&self, _: &Profile, _: &str, _: &str, _: u16, _: u16) -> adapter::Result<()> {
        Ok(())
    }
}

impl Attacher for Jobs {
    fn attach(&self, _: &Profile, id: &str) -> Command {
        self.did.lock().unwrap().push(format!("attach {id}"));
        Command::new(self.prog.lock().unwrap().as_str())
    }
}

impl Screener for Jobs {
    fn screen(&self, p: &Profile, dir: &str, command: &str, hint: &str) -> Command {
        self.did
            .lock()
            .unwrap()
            .push(format!("screen /{command} in {dir} as {} · {hint}", p.name));
        Command::new(self.prog.lock().unwrap().as_str())
    }
}

fn agent(key: &str) -> Agent {
    Agent {
        kind: Kind::from("jobs"),
        key: key.into(),
        display_name: format!("tidy {key}"),
        pid: 812,
        acct: Profile {
            kind: Kind::from("jobs"),
            name: "work".into(),
            dir: "/x/.jobs".into(),
        },
        job: Job {
            id: key.into(),
            session_id: format!("s-{key}"),
            state: State::Idle,
            cwd: "/src/app".into(),
            updated_at: 1_000,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn model(agents: Vec<Agent>) -> (Model, Jobs, Arc<Fake>) {
    let mut m = crate::screens::social::feed::tests::model(120, 30);
    let jobs = Jobs::new();
    let reg = Registry::new();
    reg.register(jobs.clone());
    m.reg = reg;
    let n = agents.len();
    m.set_fleet(crate::worker::Fleet {
        agents,
        roots: vec![String::new(); n],
        ..Default::default()
    });
    let h = Fake::new();
    set_hosts(&mut m, Arc::new(h.clone()));
    (m, jobs, h)
}

fn flash(m: &Model) -> String {
    m.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
}

fn said(m: &Model) -> String {
    m.said.last().map(|s| s.text.clone()).unwrap_or_default()
}

/// #full joins the session's screen once the key has been handled, as its
/// account; leaving it comes back with nothing said.
#[test]
fn full_joins_the_screen() {
    let (mut m, jobs, _) = model(vec![agent("j1")]);
    m.sel = "j1".into();
    m.command("#full");
    assert_eq!(jobs.did(), ["join j1 as work"], "made, not run");
    assert_eq!(m.run_outside(), 1);
    assert_eq!(jobs.did(), ["join j1 as work", "ran j1"]);
    assert_eq!(flash(&m), "");
}

/// Opened in another window, it says so; let go by its agent, it moves to
/// rush mode; anything else falls back on the agent's own attach, run
/// with the terminal too, and what that says is a failure.
#[test]
fn how_a_joined_screen_ends() {
    let (mut m, jobs, h) = model(vec![agent("j1")]);
    m.sel = "j1".into();
    *jobs.ends.lock().unwrap() = Ends::Elsewhere;
    m.command("#full");
    m.run_outside();
    assert_eq!(flash(&m), "opened in another window");

    *jobs.ends.lock().unwrap() = Ends::Gone;
    m.command("#full");
    m.run_outside();
    assert_eq!(
        *h.adopted.lock().unwrap(),
        [("j1".to_string(), String::new())]
    );
    // Moved, its row was put away and the new one waited for: picked
    // again here.
    assert!(m.agents[0].done);
    m.agents[0].done = false;
    m.rebuild();
    m.sel = "j1".into();

    *jobs.ends.lock().unwrap() = Ends::Broke;
    *jobs.prog.lock().unwrap() = "/usr/bin/false".into();
    jobs.did.lock().unwrap().clear();
    m.command("#full");
    assert_eq!(m.run_outside(), 2, "the screen, then the agent's attach");
    assert_eq!(jobs.did(), ["join j1 as work", "ran j1", "attach j1"]);
    assert_eq!(said(&m), "exit status 1");
}

/// Who can't be opened full screen says why, and nothing runs.
#[test]
fn full_says_why_not() {
    let mut term = agent("t1");
    term.interactive = true;
    let mut past = agent("p1");
    past.past = true;
    let mut rush = agent("r1");
    rush.rush = true;
    let (mut m, jobs, _) = model(vec![term.clone(), past.clone(), rush]);
    for key in ["t1", "p1", "r1"] {
        m.sel = key.into();
        m.command("#full");
        assert_eq!(
            flash(&m),
            "only a Claude Code agent in the background opens full screen"
        );
    }
    assert_eq!(super::attach::attach(&mut m, Some(term)), Out::Draw);
    assert_eq!(flash(&m), "tidy t1 is open in another terminal (pid 812)");
    super::attach::attach(&mut m, Some(past));
    assert_eq!(
        flash(&m),
        "tidy p1 is a past conversation · a message resumes it in rush mode"
    );
    m.sel = String::new();
    m.command("#full");
    assert_eq!(flash(&m), "select an agent first");
    assert_eq!(m.run_outside(), 0);
    assert!(jobs.did().is_empty());
}

/// The bar offers a background session full screen, and its row opens it.
#[test]
fn the_bar_opens_it_full_screen() {
    let (mut m, jobs, _) = model(vec![agent("j1")]);
    m.sel = "j1".into();
    let items = super::bar::bar_places(&m, "");
    let it = items
        .iter()
        .find(|i| i.title == "Open full screen")
        .expect("offered");
    assert_eq!(it.meta, "tidy j1 in Claude Code");
    let go = it.go.clone();
    super::bar::run(&mut m, &go);
    assert_eq!(m.run_outside(), 1);
    assert_eq!(jobs.did(), ["join j1 as work", "ran j1"]);
}

/// /mcp opens Claude Code's own screen in the session's folder, as its
/// account, its hint above it; one that fails says so.
#[test]
fn claude_code_screens() {
    use crate::screens::session_cards::claudescreen::{claude_screen, open_screen};
    assert_eq!(claude_screen("plugins"), Some("plugin"));
    assert_eq!(claude_screen("mcp"), Some("mcp"));
    assert_eq!(claude_screen("btw"), None);
    let (mut m, jobs, _) = model(vec![agent("j1")]);
    open_screen(&mut m, "j1", "mcp");
    assert_eq!(
        jobs.did(),
        [
            "screen /mcp in /src/app as work · \x1b[2m  rush · Jobs Code's /mcp · when you're done: esc, then ctrl+c twice to come back\x1b[0m"
        ]
    );
    assert_eq!(m.run_outside(), 1);
    assert_eq!(flash(&m), "");
    *jobs.prog.lock().unwrap() = "/usr/bin/false".into();
    open_screen(&mut m, "j1", "plugin");
    m.run_outside();
    assert_eq!(said(&m), "Claude Code's /plugin: exit status 1");
}
