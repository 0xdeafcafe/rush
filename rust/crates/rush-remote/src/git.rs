//! Read-only git in a session's folder, for a remote rush's changes view;
//! and that view's side: git for a folder an attached session works in,
//! run on its machine through its serve. After Go's internal/remote/git.go
//! (GitRequest :24, GitReply :30, gitReads :37, gitRefused :41, checkGit
//! :50, realDir :91, under :98, gitTop :103, runGit :113, limitWriter
//! :126, git :142, Git :167).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::client::peer_post;
use crate::config::load_config;
use crate::http::{Request, Response};
use crate::server::{Server, clean, fail, read_json, real, under, write_json};

/// One read-only git command in a session's folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitRequest {
    pub cwd: String,
    #[serde(deserialize_with = "rush_core::wire::nullvec::deserialize")]
    pub args: Vec<String>,
}

/// What it printed, or why it didn't run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitReply {
    #[serde(serialize_with = "ser_b64", deserialize_with = "de_b64")]
    pub out: Vec<u8>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

fn ser_b64<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
    // Go's empty buffer is a nil []byte, written as null.
    if b.is_empty() {
        return s.serialize_none();
    }
    s.serialize_str(&STANDARD.encode(b))
}

fn de_b64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    s.map_or(Ok(Vec::new()), |s| {
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    })
}

/// The git commands a remote client may run: the ones rush's changes view
/// reads the working tree and its commits with.
const GIT_READS: [&str; 10] = [
    "rev-parse",
    "diff",
    "ls-files",
    "reflog",
    "show",
    "name-rev",
    "rev-list",
    "merge-base",
    "log",
    "status",
];

/// Options that would write, run something, or look outside the folder.
const GIT_REFUSED: [&str; 11] = [
    "--output",
    "--ext-diff",
    "--textconv",
    "--exec-path",
    "--git-dir",
    "--work-tree",
    "--upload-pack",
    "--receive-pack",
    "-c",
    "-C",
    "--config",
];

const GIT_OUT_MOST: usize = 32 << 20;

/// Whether r may run here: in a folder at or under one of this machine's
/// sessions' (or its repository's top), a read from GIT_READS, with no
/// refused option and no path outside that session's repository; the
/// folder to run it in.
pub fn check_git(r: &GitRequest) -> Result<PathBuf, String> {
    if !Path::new(&r.cwd).is_absolute()
        || r.args.is_empty()
        || !GIT_READS.contains(&r.args[0].as_str())
    {
        return Err("that git command isn't available remotely".into());
    }
    for a in &r.args[1..] {
        for bad in GIT_REFUSED {
            if a == bad
                || a.starts_with(&format!("{bad}="))
                || (bad.len() == 2 && a.starts_with(bad))
            {
                return Err(format!("git {a} isn't available remotely"));
            }
        }
    }
    let cwd = real(&r.cwd);
    let mut root = None;
    for i in rush_host::list() {
        if i.is_remote() || i.cwd.is_empty() {
            continue;
        }
        let sc = real(&i.cwd);
        if under(&cwd, &sc) || (under(&sc, &cwd) && git_top(&sc).as_deref() == Some(cwd.as_path()))
        {
            root = Some(git_top(&sc).unwrap_or(sc));
            break;
        }
    }
    let Some(root) = root else {
        return Err("no session works in that folder".into());
    };
    for a in &r.args[1..] {
        if a.starts_with('-') || a == "/dev/null" {
            continue;
        }
        // A path, or a revision, which reads as one inside.
        let p = if Path::new(a).is_absolute() {
            PathBuf::from(a)
        } else {
            cwd.join(a)
        };
        if !under(&clean(&p), &root) {
            return Err(format!("{a} is outside the session's repository"));
        }
    }
    Ok(cwd)
}

/// The repository dir is in's top.
fn git_top(dir: &Path) -> Option<PathBuf> {
    let (out, err) = run_git(dir, &["rev-parse".into(), "--show-toplevel".into()]);
    if err.is_some() {
        return None;
    }
    let t = String::from_utf8_lossy(&out).trim().to_string();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

/// Runs git in dir as a look: no locks taken, no fsmonitor hook or external
/// diff run, giving up after 10 seconds. What it printed (at most 32 MB),
/// and how it failed.
pub fn run_git(dir: &Path, args: &[String]) -> (Vec<u8>, Option<String>) {
    let child = Command::new("git")
        .args(["-c", "core.fsmonitor=false", "-c", "diff.external=", "-C"])
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (Vec::new(), Some(e.to_string())),
    };
    let pid = child.id() as i32;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if rx.recv_timeout(Duration::from_secs(10)).is_err() {
            // SAFETY: kill only signals the git this started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    });
    let mut out = Vec::new();
    if let Some(so) = child.stdout.take() {
        let mut buf = [0u8; 64 << 10];
        let mut so = so;
        while let Ok(n) = so.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = GIT_OUT_MOST.saturating_sub(out.len());
            out.extend_from_slice(&buf[..n.min(room)]);
        }
    }
    let st = child.wait();
    let _ = tx.send(());
    let err = match st {
        Ok(s) if s.success() => None,
        Ok(s) => Some(match s.code() {
            Some(c) => format!("exit status {c}"),
            None => "signal: killed".into(),
        }),
        Err(e) => Some(e.to_string()),
    };
    (out, err)
}

impl Server {
    pub(crate) fn git(&self, r: &Request) -> Response {
        let req = match read_json::<GitRequest>(r, 1 << 20) {
            Ok(q) => q,
            Err(e) => return fail(400, &e),
        };
        let dir = match check_git(&req) {
            Ok(d) => d,
            Err(e) => return fail(403, &e),
        };
        let (out, err) = run_git(&dir, &req.args);
        write_json(&GitReply {
            out,
            error: err.unwrap_or_default(),
        })
    }
}

/// The folder a machine's sessions' working folders are shown under here
/// (attach.go unreachable).
pub fn unreachable(machine: &str) -> PathBuf {
    let d = crate::config::dir().join("remote-cwd").join(machine);
    std::path::absolute(&d).unwrap_or(d)
}

/// What git printed, or what it printed and how it failed.
pub type GitRun = Result<Vec<u8>, (Vec<u8>, String)>;

/// Runs git for this machine's rush view in a folder it shows an attached
/// session working in, on that session's machine, through its serve.
/// None for any other folder: it's this machine's own git. Paths cross
/// under the unreachable folder, so what the view gets back names no path
/// of this machine.
pub fn git(dir: &str, args: &[String]) -> Option<GitRun> {
    let top = crate::config::dir().join("remote-cwd");
    let top = std::path::absolute(&top).unwrap_or(top);
    let rel = Path::new(dir).strip_prefix(&top).ok()?;
    let machine = rel
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    if rel.as_os_str().is_empty() || machine == ".." {
        return None;
    }
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return Some(Err((Vec::new(), e))),
    };
    let Some(p) = cfg.peer(&machine) else {
        return Some(Err((
            Vec::new(),
            format!("{machine} isn't a machine in remote.json"),
        )));
    };
    let local = unreachable(&machine).to_string_lossy().into_owned();
    let strip = |s: &str| -> String {
        if s == local {
            return "/".into();
        }
        match s.strip_prefix(&format!("{local}/")) {
            Some(r) => format!("/{r}"),
            None => s.to_string(),
        }
    };
    let req = GitRequest {
        cwd: strip(dir),
        args: args.iter().map(|a| strip(a)).collect(),
    };
    let rep: GitReply = match peer_post(&p, "/api/git", &req, Duration::from_secs(15)) {
        Ok(r) => r,
        Err(e) => return Some(Err((Vec::new(), e))),
    };
    if !rep.error.is_empty() {
        return Some(Err((rep.out, rep.error)));
    }
    if args.first().is_some_and(|a| a == "rev-parse") && args.iter().any(|a| a == "--show-toplevel")
    {
        let t = String::from_utf8_lossy(&rep.out).trim().to_string();
        let joined = format!("{local}/{}", t.trim_start_matches('/'));
        return Some(Ok(
            format!("{}\n", clean(Path::new(&joined)).display()).into_bytes()
        ));
    }
    Some(Ok(rep.out))
}
