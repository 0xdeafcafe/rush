//! An agent rush knows only through ACP: how to start it, and where it
//! keeps its things. OpenCode needs nothing more; Kimi, Vibe, Gemini and
//! GLM build on it. Port of internal/adapters/acp/agent.go:1-285.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rush_core::adapter::{
    self, Adapter, Conn, Driver, Feature, Features, KeyChecker, Kind, Level, Made, Once,
    OnceReader, Profile, Programmer, Published, Publisher, Sink, Spawnable, StartOptions, Support,
};
use serde_json::json;

use crate::rpc::Error;
use crate::session::{Options, Session};
use crate::util::{env, home_dir, on_path, path_str};

/// How long an agent has to open its session before rush gives up on it
/// (Go's caller's context).
pub const START_WITHIN: Duration = Duration::from_secs(60);

/// The MCP server rush serves its own tools as (Go's agtools.Server).
pub const TOOLS_SERVER: &str = "rush";

/// An agent rush knows only through ACP.
#[derive(Clone, Default)]
pub struct Agent {
    pub id: Kind,
    pub title: String,
    /// Whose models it runs, when that isn't title: Google for Gemini CLI.
    pub company: String,
    /// Its program.
    pub command: String,
    /// What makes it speak ACP.
    pub args: Vec<String>,
    /// The program that does, beside command, when it's another: vibe-acp.
    pub acp: String,
    /// Its config folder, under the home folder: ".kimi-code".
    pub home: String,
    /// How command runs one task and prints the answer, "<task>" for the
    /// prompt.
    pub once: String,
    /// The flag that picks its model, or a VAR=.
    pub model: String,
    /// The flags of a one-shot run rush hosts, by what each sets: prompt,
    /// model, cwd, mode (its value), mode=m, "=v" for one that must be v,
    /// "" for one that changes nothing printed.
    pub flags: HashMap<String, String>,
    /// Files in home any of which means it's signed in.
    pub creds: Vec<String>,
    /// Environment variables any of which means it's signed in; with
    /// neither, rush takes it as it is.
    pub keys: Vec<String>,
    /// How to sign it in, when it isn't.
    pub sign_in: String,
    /// Features it has, or lacks, beyond what ACP gives every agent.
    pub more: Features,
    /// How far rush's support for it has been tried.
    pub tried: Level,
    /// Where it's published, when that's known.
    pub publ: Published,
}

fn flags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn strs(ss: &[&str]) -> Vec<String> {
    ss.iter().map(|s| s.to_string()).collect()
}

/// Kimi CLI, as ACP knows it.
pub fn kimi_agent() -> Agent {
    Agent {
        id: Kind::from("kimi"),
        title: "Kimi CLI".into(),
        company: "Moonshot".into(),
        command: "kimi".into(),
        args: strs(&["acp"]),
        home: ".kimi".into(),
        once: r#"kimi -p "<task>""#.into(),
        model: "-m".into(),
        flags: flags(&[
            ("-p", "prompt"),
            ("--prompt", "prompt"),
            ("-m", "model"),
            ("--model", "model"),
            ("--output-format", "=text"),
        ]),
        ..Agent::default()
    }
}

/// OpenCode, which needs nothing more than ACP gives.
pub fn opencode() -> Agent {
    Agent {
        id: Kind::from("opencode"),
        title: "OpenCode".into(),
        command: "opencode".into(),
        args: strs(&["acp"]),
        home: ".config/opencode".into(),
        publ: Published {
            npm: "opencode-ai".into(),
            update: strs(&["upgrade"]),
            ..Published::default()
        },
        once: r#"opencode run "<task>""#.into(),
        model: "-m".into(),
        flags: flags(&[
            ("-m", "model"),
            ("--model", "model"),
            ("--dir", "cwd"),
            ("--format", "=default"),
        ]),
        ..Agent::default()
    }
}

/// What ACP gives any agent. Rewind, fork, context breakdowns, background
/// tasks and a screen of its own have no ACP equivalent.
pub fn acp_features() -> Features {
    use Feature::*;
    let y = |n: &str| Support::YES.with(n);
    HashMap::from([
        (Run, Support::YES),
        (Resume, Support::YES),
        (Interrupt, Support::YES),
        (Model, y("when the agent offers models")),
        (Modes, y("the agent's own")),
        (Plan, y("when the agent has a plan mode")),
        (Images, Support::YES),
        (Questions, Support::YES),
        (Mcp, Support::YES),
        (HandoffIn, Support::YES),
        (Subagents, y("when the agent has a subagent tool")),
        (Background, Support::NO.with("ACP has no background tasks")),
        (History, Support::PLANNED),
        (Pricing, Support::PLANNED),
    ])
}

impl Agent {
    /// ACP's features, with the agent's own over them.
    pub fn all_features(&self) -> Features {
        let mut out = acp_features();
        for (f, s) in &self.more {
            out.insert(*f, s.clone());
        }
        out
    }

    /// Its config folder, when it's installed.
    pub fn home_profiles(&self) -> Vec<Profile> {
        if !installed(&self.id) {
            return Vec::new();
        }
        let home = home_dir();
        if home.is_empty() {
            return Vec::new();
        }
        vec![Profile {
            kind: self.id.clone(),
            name: self.title.clone(),
            dir: path_str(&Path::new(&home).join(&self.home)),
        }]
    }

    /// Whether it's signed in: a file it keeps, or a key set.
    pub fn check(&self, p: &Profile) -> Result<(), Error> {
        if self.creds.is_empty() && self.keys.is_empty() {
            return Ok(());
        }
        if self.keys.iter().any(|k| !env(k).is_empty()) {
            return Ok(());
        }
        if self
            .creds
            .iter()
            .any(|f| std::fs::metadata(Path::new(&p.dir).join(f)).is_ok())
        {
            return Ok(());
        }
        Err(Error::other(format!(
            "{} isn't signed in: {}",
            self.title, self.sign_in
        )))
    }

    /// Runs it over ACP, then puts it in the mode, model and effort asked
    /// for. finds_children: its subagents keep runs of their own, found by
    /// the call that started them.
    pub fn run(
        &self,
        o: StartOptions,
        sink: Arc<dyn Sink>,
        finds_children: bool,
    ) -> Result<Session, Error> {
        if o.fork {
            return Err(Error::no_fork());
        }
        let mut cmd = self.command.clone();
        if !o.binary.is_empty() {
            cmd = o.binary.clone();
        } else if !on_path(&cmd) {
            let p = adapter::global().path(self.id.as_str());
            if !p.is_empty() {
                cmd = p;
            }
        }
        if !self.acp.is_empty() {
            let dir = Path::new(&cmd).parent().unwrap_or(Path::new(""));
            cmd = path_str(&dir.join(&self.acp));
        }
        let mut args = self.args.clone();
        args.extend(o.flags.iter().cloned());
        let mut opts = Options {
            command: cmd,
            args,
            env: o.env.clone(),
            dir: o.dir.clone(),
            adapter: self.id.to_string(),
            finds_children,
            within: Some(START_WITHIN),
            ..Options::default()
        };
        if o.resume {
            opts.resume = o.session_id.clone();
        }
        for server in &o.tools {
            if server.name == TOOLS_SERVER && !server.args.is_empty() {
                let exe = std::env::current_exe()?;
                opts.mcp_servers.push(json!({
                    "args": server.args,
                    "command": path_str(&exe),
                    "env": tools_env(),
                    "name": server.name,
                }));
                for n in &server.trusted {
                    opts.trusted.push(format!("mcp__{}__{n}", server.name));
                }
            }
        }
        let s = Session::start(opts, sink)?;
        if !o.mode.is_empty()
            && let Err(e) = s.switch_mode(&o.mode)
        {
            s.abandon();
            return Err(e);
        }
        if !o.model.is_empty()
            && let Err(e) = s.switch_model(&o.model)
        {
            s.abandon();
            return Err(e);
        }
        // A saved effort the model doesn't offer (switched to one that
        // doesn't think) starts it at the model's own.
        if !o.effort.is_empty()
            && let Err(e) = s.set_effort(&o.effort)
            && !e.is_unsupported()
        {
            s.abandon();
            return Err(e);
        }
        Ok(s)
    }

    /// Reads args as its one-shot run: the prompt after once's flag, or the
    /// words after its subcommand, and what flags set. A flag it doesn't
    /// name, or two that set one thing two ways, runs the real one.
    pub fn once_of(&self, args: &[String]) -> Option<Once> {
        let mut o = Once::default();
        let w: Vec<&str> = self.once.split_whitespace().collect();
        let sub = if w.len() > 1 && !w[1].starts_with('-') {
            w[1] // opencode run
        } else {
            ""
        };
        let mut words: Vec<String> = Vec::new();
        let mut started = false;
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if !arg.starts_with('-') {
                if !sub.is_empty() && !started && arg == sub {
                    started = true;
                } else if !sub.is_empty() && started {
                    words.push(arg.clone());
                } else {
                    return None; // a prompt for its own screen
                }
                i += 1;
                continue;
            }
            let (name, mut val, eq) = match arg.split_once('=') {
                Some((n, v)) => (n, v.to_string(), true),
                None => (arg.as_str(), String::new(), false),
            };
            let role = self.flags.get(name)?;
            let bare = role.is_empty() || role.starts_with("mode=");
            if bare && eq {
                return None;
            }
            if !bare && !eq {
                if i + 1 >= args.len() || args[i + 1].starts_with('-') {
                    return None;
                }
                i += 1;
                val = args[i].clone();
            }
            let set: Option<&mut String> = if role == "prompt" {
                started = true;
                words.push(val.clone());
                None
            } else if role == "model" {
                Some(&mut o.model)
            } else if role == "cwd" {
                Some(&mut o.cwd)
            } else if role == "mode" {
                Some(&mut o.mode)
            } else if let Some(m) = role.strip_prefix("mode=") {
                val = m.to_string();
                Some(&mut o.mode)
            } else if let Some(must) = role.strip_prefix('=') {
                if val != must {
                    return None;
                }
                None
            } else {
                None
            };
            if let Some(set) = set {
                if !set.is_empty() && *set != val {
                    return None;
                }
                *set = val;
            }
            i += 1;
        }
        if let Some(v) = self.model.strip_suffix('=')
            && o.model.is_empty()
        {
            o.model = env(v);
        }
        o.prompt = words.join(" ");
        (started && !o.prompt.is_empty()).then_some(o)
    }
}

/// What rush's tools run as a process need of rush's environment, as ACP's
/// MCP server env: an agent may start them with almost none (Go's
/// agtools.Env).
pub fn tools_env() -> Vec<serde_json::Value> {
    ["RUSH_HOME", "RUSH_CACHE"]
        .iter()
        .filter_map(|k| {
            let v = env(k);
            (!v.is_empty()).then(|| json!({"name": k, "value": v}))
        })
        .collect()
}

/// Whether agent k is installed, as rush's registry has it.
pub(crate) fn installed(k: &Kind) -> bool {
    adapter::global().installed(k.as_str())
}

impl Adapter for Agent {
    fn kind(&self) -> Kind {
        self.id.clone()
    }
    fn name(&self) -> String {
        self.title.clone()
    }
    fn features(&self) -> Features {
        self.all_features()
    }
    fn level(&self) -> Level {
        self.tried
    }
    fn profiles(&self) -> Vec<Profile> {
        self.home_profiles()
    }
    rush_core::has!(
        driver: Driver,
        once_reader: OnceReader,
        spawnable: Spawnable,
        key_checker: KeyChecker,
        programmer: Programmer,
        made: Made,
        publisher: Publisher,
    );
}

impl Driver for Agent {
    fn start(&self, o: StartOptions, sink: Arc<dyn Sink>) -> adapter::Result<Box<dyn Conn>> {
        Ok(Box::new(self.run(o, sink, false)?))
    }
}

impl OnceReader for Agent {
    fn read_once(&self, args: &[String]) -> Option<Once> {
        self.once_of(args)
    }
}

impl Spawnable for Agent {
    fn spawn_command(&self) -> (String, String) {
        (self.once.clone(), self.model.clone())
    }
}

impl KeyChecker for Agent {
    fn check_key(&self, p: &Profile) -> adapter::Result<()> {
        Ok(self.check(p)?)
    }
}

impl Programmer for Agent {
    /// Its program, which its installer may put in its config folder's bin.
    fn program(&self) -> (String, Vec<String>) {
        (
            self.command.clone(),
            vec![path_str(&Path::new(&self.home).join("bin"))],
        )
    }
}

impl Made for Agent {
    fn maker(&self) -> String {
        self.company.clone()
    }
}

impl Publisher for Agent {
    fn published(&self) -> Published {
        self.publ.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        strs(s)
    }

    #[test]
    fn reads_one_shot_runs() {
        let k = kimi_agent();
        let o = k.once_of(&args(&["-p", "fix it", "-m", "k2"])).unwrap();
        assert_eq!((o.prompt.as_str(), o.model.as_str()), ("fix it", "k2"));
        assert!(k.once_of(&args(&["fix it"])).is_none(), "its own screen");
        assert!(
            k.once_of(&args(&["-p", "x", "--yolo"])).is_none(),
            "a flag it doesn't name"
        );
        assert!(
            k.once_of(&args(&["-p", "x", "--output-format", "json"]))
                .is_none()
        );
        assert!(
            k.once_of(&args(&["-p", "x", "--output-format=text"]))
                .is_some()
        );
        assert!(
            k.once_of(&args(&["-m", "a", "-m", "b", "-p", "x"]))
                .is_none(),
            "two models"
        );
        let oc = opencode();
        let o = oc
            .once_of(&args(&["run", "--dir", "/w", "say", "hi"]))
            .unwrap();
        assert_eq!((o.prompt.as_str(), o.cwd.as_str()), ("say hi", "/w"));
        assert!(oc.once_of(&args(&["say"])).is_none());
    }

    // checkkey_test.go: TestCheckKey
    #[test]
    fn check_key() {
        let g = Agent {
            id: Kind::from("x"),
            title: "X".into(),
            creds: strs(&["oauth_creds.json"]),
            keys: strs(&["RUSH_ACP_TEST_KEY_NEVER_SET"]),
            ..Agent::default()
        };
        let d = tempfile::tempdir().unwrap();
        let p = Profile {
            dir: path_str(d.path()),
            ..Profile::default()
        };
        assert!(
            g.check(&p).is_err(),
            "no creds and no key reads as signed in"
        );
        std::fs::write(d.path().join("oauth_creds.json"), "{}").unwrap();
        assert!(g.check(&p).is_ok());
        let d2 = tempfile::tempdir().unwrap();
        let p2 = Profile {
            dir: path_str(d2.path()),
            ..Profile::default()
        };
        assert!(kimi_agent().check(&p2).is_ok(), "kimi has no check");
    }

    #[test]
    fn a_fork_is_refused() {
        struct Nowhere;
        impl Sink for Nowhere {
            fn event(&self, _: rush_core::event::Event) {}
            fn ended(&self, _: Option<adapter::Error>) {}
        }
        let a = Agent {
            id: Kind::from("x"),
            command: "x".into(),
            ..Agent::default()
        };
        let o = StartOptions {
            fork: true,
            ..StartOptions::default()
        };
        let err = a.run(o, Arc::new(Nowhere), false).err().unwrap();
        assert_eq!(err.kind(), &crate::rpc::ErrorKind::NoFork);
        assert_eq!(err.to_string(), "acp: this agent can't fork a session");
    }
}
