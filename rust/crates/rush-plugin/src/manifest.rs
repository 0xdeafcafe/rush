//! plugin.json: what a plugin is, what it asks for, and checking it.
//! Ported from internal/plugin/plugin.go:67-460 (Manifest, Requires,
//! Validate, SplitHostPort, ExecArgv, WorkspaceDirs) and category.go.

use std::collections::BTreeMap;

use rush_state::json::RawJson;
use serde::{Deserialize, Serialize};

use crate::cli::CliSpec;
use crate::gopath;
use crate::jsonshape::{self, Shape};
use crate::text::go_q;
use crate::ui::{CommandSpec, SettingSpec};

/// Protocols a plugin can speak.
pub const PROTO_RUSH: &str = "rush"; // framed JSON-RPC on fd 3; the default
pub const PROTO_MCP: &str = "mcp"; // an MCP server on stdio

// Capabilities a plugin can be given over sessions.

/// Lists every rush-mode session, and follows the list as it changes:
/// each one's name, folder, repo and branch, state, what it's doing, its
/// cost and context. Not what was said.
pub const CAP_LIST: &str = "list";
/// Starts sessions in the manifest's workspaces.
pub const CAP_START: &str = "start";
/// Follows what the plugin's own sessions say.
pub const CAP_READ: &str = "read";
/// Sends messages to the plugin's own sessions.
pub const CAP_SEND: &str = "send";
/// Interrupts and stops the plugin's own sessions.
pub const CAP_CONTROL: &str = "control";
/// Queues messages to any session in the manifest's workspaces, as the
/// plugin, never one that skips asking you. It goes when the session's
/// turn ends, as a queued message of yours does.
pub const CAP_QUEUE: &str = "queue";
/// Sends now, or drops, a message waiting in the queue of any session in
/// the manifest's workspaces, by its place: the plugin never sees the text.
pub const CAP_QUEUED: &str = "queued";

pub(crate) const CAPS: [&str; 7] = [
    CAP_LIST,
    CAP_START,
    CAP_READ,
    CAP_SEND,
    CAP_CONTROL,
    CAP_QUEUE,
    CAP_QUEUED,
];

/// The memory limit a manifest doesn't set.
pub const DEFAULT_MEMORY_MB: u64 = 256;

// Limits on what a manifest can carry into every session.
pub(crate) const MAX_PROMPT: usize = 16 << 10;
pub(crate) const MAX_AGENTS: usize = 64 << 10;

/// Starts every plugin's MCP server name.
pub const SERVER_PREFIX: &str = "rush-";

/// plugin.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// What it's for, so rush lists it with plugins that do the same kind
    /// of thing: see CATEGORIES. It grants nothing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// The program and its arguments. The program is a path in the
    /// plugin's folder, or an absolute one (an interpreter, say).
    #[serde(serialize_with = "nil_if_empty")]
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub protocol: String,
    /// ${DATA} and ${PLUGIN} are expanded.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Offers the plugin's tools to every rush-mode session, as
    /// mcp__rush-<name>__<tool>. They ask before running, like any tool.
    /// An MCP plugin always offers them.
    #[serde(skip_serializing_if = "is_false")]
    pub tools: bool,
    /// Capabilities over sessions, see CAP_*.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    /// The folders sessions it starts may run in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// The host:port pairs it may reach, through rush's proxy.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    /// More paths it may read, for an interpreter's libraries or another
    /// tool's files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    /// More paths it may write (and read), another tool's files say,
    /// beyond its data folder.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
    /// Programs rush runs for it, outside the sandbox, as you: each a fixed
    /// command line the plugin may add arguments to.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub exec: BTreeMap<String, Vec<String>>,
    /// Its memory limit; 0 is DEFAULT_MEMORY_MB.
    #[serde(rename = "memoryMB", skip_serializing_if = "is_zero")]
    pub memory_mb: i64,
    /// Subagents given to every rush-mode session, as Claude Code's
    /// --agents takes them; each is named <plugin>:<agent>.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, RawJson>,
    /// Added to every rush-mode session's system prompt.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// It may arrange rush's agent list with sidebar.set.
    #[serde(skip_serializing_if = "is_false")]
    pub sidebar: bool,
    /// What it may do in rush's own screen. See UI_*.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<String>,
    /// Commands it adds to the # commands, the command bar and the keymap.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandSpec>,
    /// Settings it offers under Settings, Plugins.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<SettingSpec>,
    /// The commands it adds to rush's CLI.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cli: Vec<CliSpec>,
    /// What the system must offer for it to run at all, on top of being
    /// approved.
    #[serde(skip_serializing_if = "Requires::is_empty")]
    pub requires: Requires,
    /// For a plugin a repository ships, more paths in that repository it's
    /// built from, relative to its folder there.
    #[serde(rename = "builtFrom", skip_serializing_if = "Vec::is_empty")]
    pub built_from: Vec<String>,
    /// The services it answers for other plugins and rush, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    /// The services it may ask other plugins for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<String>,
}

/// Requires gates whether a plugin can run here. Empty fields don't
/// restrict; where several values are given, any one of them is enough,
/// but every field given must be satisfied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Requires {
    /// GOOS values it runs on, e.g. "darwin", "linux".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    /// GOARCH values it runs on, e.g. "arm64", "amd64".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arch: Vec<String>,
    /// Programs it needs on PATH.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<String>,
}

impl Requires {
    pub fn is_empty(&self) -> bool {
        self.os.is_empty() && self.arch.is_empty() && self.bin.is_empty()
    }

    /// Checks the shape: bin names are bare program names, not paths,
    /// since unmet only ever looks them up on PATH.
    fn validate(&self) -> Result<(), String> {
        for b in &self.bin {
            if b.is_empty() || *b != gopath::base(b) {
                return Err(format!(
                    "requires.bin {}: use a bare program name, found on PATH",
                    go_q(b)
                ));
            }
        }
        Ok(())
    }

    /// r in words, "" when it asks for nothing: "macOS or Linux, git on
    /// PATH".
    pub fn needs(&self) -> String {
        let mut needs = Vec::new();
        if !self.os.is_empty() {
            needs.push(os_words(&self.os));
        }
        if !self.arch.is_empty() {
            needs.push(self.arch.join(" or "));
        }
        for b in &self.bin {
            needs.push(format!("{b} on PATH"));
        }
        needs.join(", ")
    }
}

fn nil_if_empty<S: serde::Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_none();
    }
    v.serialize(s)
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

const ANY: Shape = Shape::Any;
const REQUIRES_SHAPE: Shape = Shape::Struct(
    "plugin.Requires",
    &[("os", ANY), ("arch", ANY), ("bin", ANY)],
);
const COMMAND_SHAPE: Shape = Shape::Struct(
    "plugin.CommandSpec",
    &[("name", ANY), ("description", ANY), ("key", ANY)],
);
const SETTING_SHAPE: Shape = Shape::Struct(
    "plugin.SettingSpec",
    &[
        ("key", ANY),
        ("title", ANY),
        ("description", ANY),
        ("type", ANY),
        ("choices", ANY),
        ("default", ANY),
    ],
);
const CLI_SHAPE: Shape = Shape::Struct(
    "plugin.CLISpec",
    &[("name", ANY), ("usage", ANY), ("description", ANY)],
);
pub(crate) const MANIFEST_SHAPE: Shape = Shape::Struct(
    "plugin.Manifest",
    &[
        ("name", ANY),
        ("description", ANY),
        ("version", ANY),
        ("category", ANY),
        ("command", ANY),
        ("protocol", ANY),
        ("env", ANY),
        ("tools", ANY),
        ("sessions", ANY),
        ("workspaces", ANY),
        ("network", ANY),
        ("read", ANY),
        ("write", ANY),
        ("exec", ANY),
        ("memoryMB", ANY),
        ("agents", ANY),
        ("prompt", ANY),
        ("sidebar", ANY),
        ("ui", ANY),
        ("commands", Shape::Seq(&COMMAND_SHAPE)),
        ("settings", Shape::Seq(&SETTING_SHAPE)),
        ("cli", Shape::Seq(&CLI_SHAPE)),
        ("requires", REQUIRES_SHAPE),
        ("builtFrom", ANY),
        ("provides", ANY),
        ("uses", ANY),
    ],
);

impl Manifest {
    /// Reads a manifest as Go's jsonx does; strict refuses a name no field
    /// takes, as plugin.Load does.
    pub fn parse(b: &[u8], strict: bool) -> Result<Manifest, String> {
        jsonshape::decode(b, &MANIFEST_SHAPE, strict)
    }

    /// The protocol it speaks.
    pub fn proto(&self) -> &str {
        if self.protocol.is_empty() || self.protocol == "agtop" {
            // as it was called before the rename
            return PROTO_RUSH;
        }
        &self.protocol
    }

    /// Whether it offers tools to sessions.
    pub fn has_tools(&self) -> bool {
        self.tools || self.proto() == PROTO_MCP
    }

    /// Whether it was given capability c.
    pub fn can(&self, c: &str) -> bool {
        self.sessions.iter().any(|s| s == c)
    }

    /// Its memory limit in bytes.
    pub fn memory(&self) -> u64 {
        if self.memory_mb <= 0 {
            return DEFAULT_MEMORY_MB << 20;
        }
        (self.memory_mb as u64) << 20
    }

    /// The MCP server name its tools are offered under.
    pub fn server(&self) -> String {
        format!("{SERVER_PREFIX}{}", self.name)
    }

    /// Why its requirements aren't met here, "" when they are.
    pub fn unmet(&self) -> String {
        let r = &self.requires;
        if !r.os.is_empty() && !r.os.iter().any(|o| o == GOOS) {
            return format!("needs {}", os_words(&r.os));
        }
        if !r.arch.is_empty() && !r.arch.iter().any(|a| a == GOARCH) {
            return format!("needs {} (this is {GOARCH})", r.arch.join(" or "));
        }
        for bin in &r.bin {
            if look_path(bin).is_none() {
                return format!("needs {bin} on PATH");
            }
        }
        String::new()
    }

    /// The command line of the program the manifest names, with ~
    /// expanded, or None.
    pub fn exec_argv(&self, name: &str) -> Option<Vec<String>> {
        let argv = self.exec.get(name)?;
        let mut out = vec![expand_home(argv.first()?)];
        out.extend(argv[1..].iter().cloned());
        Some(out)
    }

    /// The manifest's workspaces with ~ expanded and symlinks resolved;
    /// one that doesn't exist is left out.
    pub fn workspace_dirs(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .filter_map(|w| eval_symlinks(&expand_home(w)))
            .collect()
    }

    /// The category it's shown under.
    pub fn category_of(&self) -> &'static Category {
        category_named(&self.category)
    }

    /// Checks a manifest read from dir.
    pub fn validate(&self, dir: &str) -> Result<(), String> {
        if !name_ok(&self.name) {
            return Err(format!(
                "name {}: use lowercase letters, digits and dashes, starting with a letter",
                go_q(&self.name)
            ));
        }
        if gopath::base(dir) != self.name {
            return Err(format!(
                "name {} does not match its folder {}",
                go_q(&self.name),
                go_q(&gopath::base(dir))
            ));
        }
        if self.command.first().is_none_or(|c| c.is_empty()) {
            return Err("command is empty".into());
        }
        for b in &self.built_from {
            if b.is_empty() || gopath::is_abs(b) {
                return Err(format!(
                    "builtFrom {}: use a path relative to the plugin's folder in its repository",
                    go_q(b)
                ));
            }
        }
        if !gopath::is_abs(&self.command[0]) {
            let p = gopath::clean(&self.command[0]);
            if p == ".." || p.starts_with("../") {
                return Err(format!(
                    "command {} is outside the plugin's folder",
                    go_q(&self.command[0])
                ));
            }
        }
        match self.proto() {
            PROTO_RUSH => {}
            PROTO_MCP => {
                if !self.sessions.is_empty() {
                    return Err("an MCP plugin cannot be given session capabilities: it has no way to ask for them".into());
                }
            }
            _ => {
                return Err(format!(
                    "protocol {}: use {} or {}",
                    go_q(&self.protocol),
                    go_q(PROTO_RUSH),
                    go_q(PROTO_MCP)
                ));
            }
        }
        for c in &self.sessions {
            if !CAPS.contains(&c.as_str()) {
                return Err(format!(
                    "session capability {}: use one of {}",
                    go_q(c),
                    CAPS.join(", ")
                ));
            }
        }
        if self.can(CAP_START) && self.workspaces.is_empty() {
            return Err(r#""start" needs at least one workspace to start sessions in"#.into());
        }
        for w in &self.workspaces {
            if !gopath::is_abs(&expand_home(w)) {
                return Err(format!("workspace {} is not an absolute path", go_q(w)));
            }
            if gopath::clean(&expand_home(w)) == "/" {
                return Err("a workspace cannot be /".into());
            }
        }
        if self.can(CAP_QUEUE) && self.workspaces.is_empty() {
            return Err(
                r#""queue" needs at least one workspace whose sessions it may queue to"#.into(),
            );
        }
        if self.can(CAP_QUEUED) && self.workspaces.is_empty() {
            return Err(r#""queued" needs at least one workspace whose sessions' queues it may send or drop"#.into());
        }
        for r in &self.read {
            if !gopath::is_abs(&expand_home(r)) {
                return Err(format!("read path {} is not absolute", go_q(r)));
            }
        }
        for w in &self.write {
            if !gopath::is_abs(&expand_home(w)) {
                return Err(format!("write path {} is not absolute", go_q(w)));
            }
            let c = gopath::clean(&expand_home(w));
            if c == "/" || c == gopath::clean(&expand_home("~")) {
                return Err(format!("write path {} is too broad", go_q(w)));
            }
        }
        if !self.exec.is_empty() && self.proto() != PROTO_RUSH {
            return Err(
                "an MCP plugin cannot be given programs to run: it has no way to ask for them"
                    .into(),
            );
        }
        for (name, argv) in &self.exec {
            if !agent_name_ok(name) {
                return Err(format!(
                    "exec name {}: use lowercase letters, digits and dashes",
                    go_q(name)
                ));
            }
            if argv
                .first()
                .is_none_or(|p| !gopath::is_abs(&expand_home(p)))
            {
                return Err(format!(
                    "exec {}: the program must be an absolute path",
                    go_q(name)
                ));
            }
        }
        for n in &self.network {
            split_host_port(n)?;
        }
        for k in self.env.keys() {
            if k.is_empty() || k.contains(['=', '\0']) {
                return Err(format!("env name {} is not valid", go_q(k)));
            }
        }
        if self.memory_mb < 0 || self.memory_mb > 8192 {
            return Err(format!("memoryMB {}: use 1 to 8192", self.memory_mb));
        }
        if self.sidebar && self.proto() != PROTO_RUSH {
            return Err(
                "an MCP plugin cannot be given the sidebar: it has no way to set it".into(),
            );
        }
        self.validate_ui()?;
        self.validate_cli()?;
        self.validate_category()?;
        self.requires.validate()?;
        self.validate_services()?;
        if self.prompt.len() > MAX_PROMPT {
            return Err(format!("prompt is over {MAX_PROMPT} bytes"));
        }
        let mut size = 0;
        for (name, a) in &self.agents {
            if !agent_name_ok(name) {
                return Err(format!(
                    "agent name {}: use lowercase letters, digits and dashes",
                    go_q(name)
                ));
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Def {
                description: String,
                prompt: String,
            }
            const DEF: Shape = Shape::Struct("", &[("description", ANY), ("prompt", ANY)]);
            match jsonshape::decode_str::<Def>(a.get(), &DEF, false) {
                Ok(d) if !d.description.is_empty() && !d.prompt.is_empty() => {}
                _ => {
                    return Err(format!(
                        "agent {} needs a description and a prompt",
                        go_q(name)
                    ));
                }
            }
            size += a.get().len();
        }
        if size > MAX_AGENTS {
            return Err(format!("agents are over {MAX_AGENTS} bytes"));
        }
        Ok(())
    }

    pub(crate) fn validate_category(&self) -> Result<(), String> {
        if self.category.is_empty() || CATEGORIES.iter().any(|c| c.id == self.category) {
            return Ok(());
        }
        let ids: Vec<&str> = CATEGORIES.iter().map(|c| c.id).collect();
        Err(format!(
            "category {}: use one of {}",
            go_q(&self.category),
            ids.join(", ")
        ))
    }
}

/// The OS rush runs on, as Go's runtime.GOOS names it.
pub const GOOS: &str = if cfg!(target_os = "macos") {
    "darwin"
} else {
    std::env::consts::OS
};

/// The architecture rush runs on, as Go's runtime.GOARCH names it.
pub const GOARCH: &str = match std::env::consts::ARCH.as_bytes() {
    b"aarch64" => "arm64",
    b"x86_64" => "amd64",
    b"x86" => "386",
    _ => std::env::consts::ARCH,
};

/// GOOS values as people say them.
pub fn os_name(os: &str) -> Option<&'static str> {
    match os {
        "darwin" => Some("macOS"),
        "linux" => Some("Linux"),
        "windows" => Some("Windows"),
        "freebsd" => Some("FreeBSD"),
        _ => None,
    }
}

fn os_words(os: &[String]) -> String {
    os.iter()
        .map(|o| os_name(o).map_or_else(|| o.clone(), str::to_string))
        .collect::<Vec<_>>()
        .join(" or ")
}

thread_local! {
    /// Stands in for PATH lookups in this thread's tests.
    static LOOK_PATH: std::cell::RefCell<Option<fn(&str) -> Option<String>>> = const { std::cell::RefCell::new(None) };
}

/// Replaces finding programs on PATH for this thread: for tests that check
/// unmet without touching the real PATH. None puts it back.
pub fn set_thread_look_path(f: Option<fn(&str) -> Option<String>>) {
    LOOK_PATH.with(|l| *l.borrow_mut() = f);
}

/// Finds a program on PATH, as Go's exec.LookPath: a regular file with an
/// execute bit, in the first folder of PATH that has one.
pub fn look_path(bin: &str) -> Option<String> {
    if let Some(f) = LOOK_PATH.with(|l| *l.borrow()) {
        return f(bin);
    }
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &str| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if bin.contains('/') {
        return exe(bin).then(|| bin.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for d in path.split(':') {
        let d = if d.is_empty() { "." } else { d };
        let p = format!("{d}/{bin}");
        if exe(&p) {
            return Some(p);
        }
    }
    None
}

/// The plugin behind an MCP server name, if it is one.
pub fn name_of(server: &str) -> Option<&str> {
    server.strip_prefix(SERVER_PREFIX)
}

/// Reads a network entry, host:port: the host in lower case and the port.
pub fn split_host_port(s: &str) -> Result<(String, u16), String> {
    let Some((h, p)) = go_split_host_port(s) else {
        return Err(format!("network {}: use host:port", go_q(s)));
    };
    let port = match p.parse::<i64>() {
        Ok(n) if (1..=65535).contains(&n) => n as u16,
        _ => return Err(format!("network {}: bad port", go_q(s))),
    };
    if !host_ok(h) {
        return Err(format!(
            "network {}: bad host (wildcards aren't allowed)",
            go_q(s)
        ));
    }
    Ok((h.to_lowercase(), port))
}

/// Go's net.SplitHostPort: "host:port" or "[host]:port".
fn go_split_host_port(s: &str) -> Option<(&str, &str)> {
    let i = s.rfind(':')?;
    if let Some(rest) = s.strip_prefix('[') {
        let end = rest.find(']')?;
        if end + 2 != i + 1 {
            return None; // ] must come just before the last :
        }
        let host = &rest[..end];
        if host.contains('[') || host.contains(']') {
            return None;
        }
        return Some((host, &s[i + 1..]));
    }
    let host = &s[..i];
    if host.contains(':') || host.contains('[') || host.contains(']') {
        return None;
    }
    if s[i + 1..].contains(['[', ']']) {
        return None;
    }
    Some((host, &s[i + 1..]))
}

fn lower_dash(s: &str, max_rest: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.len() - 1 <= max_rest
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// ^[a-z][a-z0-9-]{0,30}$: a plugin's name, and a command's or setting's.
pub(crate) fn name_ok(s: &str) -> bool {
    lower_dash(s, 30)
}

/// ^[a-z][a-z0-9-]{0,40}$: an agent's name, and an exec's.
pub(crate) fn agent_name_ok(s: &str) -> bool {
    lower_dash(s, 40)
}

/// ^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$
fn host_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let an = |c: &u8| c.is_ascii_alphanumeric();
    match b.len() {
        0 => false,
        1 => an(&b[0]),
        n => {
            n <= 253
                && an(&b[0])
                && an(&b[n - 1])
                && b[1..n - 1]
                    .iter()
                    .all(|c| an(c) || *c == b'.' || *c == b'-')
        }
    }
}

/// ~ or ~/… made your home folder, as Go's filepath.Join(home, p[1:]).
pub fn expand_home(p: &str) -> String {
    if p == "~" || p.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        return gopath::join(&[&home, &p[1..]]);
    }
    p.to_string()
}

/// filepath.EvalSymlinks: the path with every link followed, None when
/// it isn't there.
pub(crate) fn eval_symlinks(p: &str) -> Option<String> {
    std::fs::canonicalize(p).ok().map(|r| gopath::s(&r))
}

/// A Category says what a plugin is for, so Settings, Plugins and `rush
/// plugin list` can put plugins that do the same kind of thing together.
/// It's only where it's shown. One that names none is shown under Other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,    // as plugin.json names it
    pub title: &'static str, // as rush shows it
    pub about: &'static str, // what the plugins under it do, in a few words
}

/// Where a plugin that names no category goes.
pub const CATEGORY_OTHER: &str = "other";

/// Every category, in the order rush shows them.
pub const CATEGORIES: [Category; 10] = [
    cat(
        "messages",
        "Messages",
        "what you type and send: drafts, the queue",
    ),
    cat("voice", "Voice", "talk instead of typing"),
    cat(
        "agents",
        "Agents",
        "start, follow, arrange and continue agents",
    ),
    cat("code", "Code", "help agents read and change code"),
    cat(
        "pull-requests",
        "Pull requests",
        "follow and drive pull requests and their checks",
    ),
    cat(
        "machine",
        "Your machine",
        "share its CPU and memory between agents, and clean up its disk",
    ),
    cat("views", "Views", "how rush shows what agents do"),
    cat("memory", "Memory", "what agents remember across sessions"),
    cat(
        "integrations",
        "Other tools",
        "connect rush to other apps and services",
    ),
    cat(CATEGORY_OTHER, "Other", "plugins that name no category"),
];

const fn cat(id: &'static str, title: &'static str, about: &'static str) -> Category {
    Category { id, title, about }
}

/// The category with id, Other if there's none.
pub fn category_named(id: &str) -> &'static Category {
    CATEGORIES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or(&CATEGORIES[CATEGORIES.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(s: &str) -> RawJson {
        RawJson(serde_json::value::RawValue::from_string(s.into()).unwrap())
    }

    #[test]
    fn validate() {
        let ok = Manifest {
            name: "p".into(),
            command: vec!["bin".into()],
            ..Default::default()
        };
        type Edit = fn(&mut Manifest);
        let cases: Vec<(&str, Edit, &str)> = vec![
            ("minimal", |_| {}, ""),
            ("bad name", |m| m.name = "P!".into(), "name"),
            ("no command", |m| m.command = vec![], "command is empty"),
            (
                "command escapes",
                |m| m.command = vec!["../../bin/sh".into()],
                "outside",
            ),
            (
                "absolute interpreter",
                |m| m.command = vec!["/usr/bin/python3".into(), "main.py".into()],
                "",
            ),
            (
                "unknown protocol",
                |m| m.protocol = "grpc".into(),
                "protocol",
            ),
            (
                "mcp with sessions",
                |m| {
                    m.protocol = PROTO_MCP.into();
                    m.sessions = vec![CAP_LIST.into()];
                },
                "MCP plugin",
            ),
            (
                "unknown capability",
                |m| m.sessions = vec!["root".into()],
                "capability",
            ),
            (
                "start without workspace",
                |m| m.sessions = vec![CAP_START.into()],
                "workspace",
            ),
            (
                "workspace /",
                |m| {
                    m.sessions = vec![CAP_START.into()];
                    m.workspaces = vec!["/".into()];
                },
                "cannot be /",
            ),
            (
                "relative workspace",
                |m| m.workspaces = vec!["src".into()],
                "absolute",
            ),
            (
                "home workspace",
                |m| {
                    m.sessions = vec![CAP_START.into()];
                    m.workspaces = vec!["~/src".into()];
                },
                "",
            ),
            (
                "network wildcard",
                |m| m.network = vec!["*.example.com:443".into()],
                "wildcards",
            ),
            (
                "network no port",
                |m| m.network = vec!["example.com".into()],
                "host:port",
            ),
            (
                "network ok",
                |m| m.network = vec!["api.example.com:443".into()],
                "",
            ),
            (
                "agent without prompt",
                |m| {
                    m.agents = [("x".to_string(), raw(r#"{"description":"d"}"#))].into();
                },
                "prompt",
            ),
            (
                "agent bad name",
                |m| {
                    m.agents = [(
                        "X Y".to_string(),
                        raw(r#"{"description":"d","prompt":"p"}"#),
                    )]
                    .into();
                },
                "agent name",
            ),
            (
                "huge prompt",
                |m| m.prompt = "x".repeat(MAX_PROMPT + 1),
                "prompt",
            ),
            (
                "queue without workspace",
                |m| m.sessions = vec![CAP_QUEUE.into()],
                "workspace",
            ),
            (
                "relative write",
                |m| m.write = vec!["data".into()],
                "absolute",
            ),
            ("write home", |m| m.write = vec!["~".into()], "too broad"),
            ("write ok", |m| m.write = vec!["~/.kanban-code".into()], ""),
            (
                "exec relative",
                |m| m.exec = [("kanban".to_string(), vec!["kanban".to_string()])].into(),
                "absolute",
            ),
            (
                "exec bad name",
                |m| m.exec = [("K B".to_string(), vec!["/bin/echo".to_string()])].into(),
                "exec name",
            ),
            (
                "exec from mcp",
                |m| {
                    m.protocol = PROTO_MCP.into();
                    m.exec = [("e".to_string(), vec!["/bin/echo".to_string()])].into();
                },
                "MCP plugin",
            ),
            ("sidebar ok", |m| m.sidebar = true, ""),
            (
                "sidebar from mcp",
                |m| {
                    m.protocol = PROTO_MCP.into();
                    m.sidebar = true;
                },
                "sidebar",
            ),
            (
                "exec ok",
                |m| {
                    m.exec = [(
                        "kanban".to_string(),
                        vec!["~/.local/bin/kanban".to_string(), "--json".to_string()],
                    )]
                    .into();
                },
                "",
            ),
            (
                "requires bin path",
                |m| m.requires.bin = vec!["/usr/bin/rg".into()],
                "requires.bin",
            ),
            (
                "requires ok",
                |m| {
                    m.requires = Requires {
                        os: vec!["darwin".into(), "linux".into()],
                        bin: vec!["rg".into()],
                        ..Default::default()
                    };
                },
                "",
            ),
        ];
        for (name, edit, want) in cases {
            let mut m = ok.clone();
            edit(&mut m);
            let err = m.validate(&format!("/x/{}", m.name));
            match (want, err) {
                ("", Err(e)) => panic!("{name}: want valid, got {e}"),
                (w, Ok(())) if !w.is_empty() => panic!("{name}: want error with {w:?}, got none"),
                (w, Err(e)) if !e.contains(w) => panic!("{name}: want error with {w:?}, got {e}"),
                _ => {}
            }
        }
        assert!(
            ok.validate("/x/other").is_err(),
            "a name that doesn't match its folder was accepted"
        );
    }

    #[test]
    fn category() {
        let mut m = Manifest {
            name: "x".into(),
            command: vec!["x".into()],
            category: "voice".into(),
            ..Default::default()
        };
        assert!(m.validate("/p/x").is_ok());
        assert_eq!(m.category_of().title, "Voice");
        m.category = "telepathy".into();
        assert!(m.validate("/p/x").unwrap_err().contains("use one of"));
        m.category = String::new();
        assert!(m.validate("/p/x").is_ok());
        assert_eq!(m.category_of().id, CATEGORY_OTHER);
    }

    #[test]
    fn unmet() {
        assert_eq!(Manifest::default().unmet(), "");
        let with = |r: Requires| Manifest {
            requires: r,
            ..Default::default()
        };
        assert_ne!(
            with(Requires {
                os: vec!["never-an-os".into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        assert_eq!(
            with(Requires {
                os: vec![GOOS.into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        assert_ne!(
            with(Requires {
                arch: vec!["never-an-arch".into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        assert_eq!(
            with(Requires {
                arch: vec![GOARCH.into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        assert_ne!(
            with(Requires {
                bin: vec!["never-a-real-binary-xyz".into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        set_thread_look_path(Some(|_| Some("/bin/found".into())));
        assert_eq!(
            with(Requires {
                bin: vec!["anything".into()],
                ..Default::default()
            })
            .unmet(),
            ""
        );
        set_thread_look_path(None);
        assert_eq!(
            with(Requires {
                os: vec!["darwin".into(), "linux".into()],
                bin: vec!["git".into()],
                ..Default::default()
            })
            .requires
            .needs(),
            "macOS or Linux, git on PATH"
        );
    }

    #[test]
    fn host_port() {
        assert_eq!(
            split_host_port("API.example.com:443").unwrap(),
            ("api.example.com".into(), 443)
        );
        for bad in [
            "example.com",
            "x:0",
            "x:65536",
            "[::1]:443",
            "*.x.com:1",
            "a:b:1",
            "x:",
        ] {
            assert!(split_host_port(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn writes_as_go() {
        let m = Manifest {
            name: "p".into(),
            command: vec!["bin".into()],
            memory_mb: 64,
            agents: [("r".to_string(), raw(r#"{ "b": 1, "a": 2 }"#))].into(),
            ..Default::default()
        };
        let b = rush_state::json::to_vec(&m).unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            r#"{"name":"p","command":["bin"],"memoryMB":64,"agents":{"r":{"b":1,"a":2}}}"#
        );
        let back = Manifest::parse(
            br#"{"NAME":"p","command":["bin"],"tools":null,"memoryMB":64}"#,
            true,
        )
        .unwrap();
        assert_eq!((back.name.as_str(), back.memory_mb), ("p", 64));
    }
}
