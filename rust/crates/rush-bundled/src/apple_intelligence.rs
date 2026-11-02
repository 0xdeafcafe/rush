//! The bundled plugin that answers rush's small jobs (session titles,
//! feed summaries, quick ask) with Apple Intelligence's on-device model,
//! through `fm`, the command macOS 27 ships for it. It offers one model on
//! the plugin protocol's models.list and models.ask. Off until you turn
//! it on; where there's no `fm` (Linux, older macOS) its model says it
//! can't be used, and rush asks Claude haiku or Ollama instead. No Go
//! original: new in the Rust port, replacing a Swift plugin.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rush_plugin::models::{Answer, Ask, Model, ModelList};
use rush_plugin::rpc::{CODE_INVALID_PARAMS, from_raw, no_method};
use rush_plugin::{Bundle, Ctx, Error, FixAction, Manifest, Readiness};

/// The plugin's name, which its switch is kept under.
pub const NAME: &str = "apple-intelligence";
/// What models.ask calls its one model.
pub const MODEL: &str = "on-device";
/// What rush shows for it.
pub const LABEL: &str = "Apple on-device model";
/// How many tokens the model holds, prompt and answer together.
pub const CONTEXT_TOKENS: i64 = 4096;
/// System Settings' Apple Intelligence & Siri page.
pub const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Siri-Settings.extension";

/// Why there's no model where `fm` is missing: models.list's why.
pub const NEEDS_MACOS: &str = "needs macOS 27 or later (Apple's fm tool)";

/// What fm says on stderr when the input doesn't fit.
const CONTEXT_EXCEEDED: &str = "exceeded the model's context size";
/// What `fm available` prints when the model can be used.
const AVAILABLE: &str = "System model available";

/// How long a check (fm available, fm license --status) may take.
const CHECK_WAIT: Duration = Duration::from_secs(10);
/// How long an answer may take: a little under the broker's wait.
const ASK_WAIT: Duration = Duration::from_secs(55);
/// How long models.list trusts the last check, so a title doesn't run
/// fm every time.
const CACHE_FOR: Duration = Duration::from_secs(60);

/// The plugin's.
pub fn manifest() -> Manifest {
    Manifest {
        models: true,
        ..crate::manifest(
            NAME,
            "other",
            "Answers rush's small jobs (session titles, feed summaries, quick ask) with Apple Intelligence's \
             on-device model: free, private and offline. Needs macOS 27, with Apple Intelligence on.",
            32,
        )
    }
}

pub(crate) fn bundle() -> Bundle {
    Bundle {
        check: Some(Arc::new(|| check().readiness())),
        ..crate::bundle_of(manifest(), true, run)
    }
}

/// Where the on-device model stands here, in the order to fix it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No fm: not macOS 27 or later.
    NoFm,
    /// Apple's terms for the model aren't agreed yet.
    NoLicense,
    /// fm says the model can't be used: Apple Intelligence off, the model
    /// still downloading, a Mac that can't run it.
    Unavailable,
    Ready,
}

/// Where the model stands, in plain words, with what to open to fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// One line for you: what's missing and how to fix it, or what it
    /// now does.
    pub message: String,
    /// models.list's why: short, for when it isn't available.
    pub why: String,
    /// System Settings' page, when that's where the fix is.
    pub action: Option<FixAction>,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.state == State::Ready
    }

    /// As any bundled plugin says it.
    pub fn readiness(&self) -> Readiness {
        Readiness {
            ready: self.ready(),
            message: self.message.clone(),
            action: self.action.clone(),
        }
    }

    /// Its model, as models.list says it.
    pub fn model(&self) -> Model {
        Model {
            name: MODEL.into(),
            label: LABEL.into(),
            context_tokens: CONTEXT_TOKENS,
            available: self.ready(),
            why: if self.ready() {
                String::new()
            } else {
                self.why.clone()
            },
        }
    }
}

/// What a run of fm said; None from the runner when it couldn't start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Checks, in order, whether fm is here, Apple's terms are agreed, and
/// the model can be used. It runs fm (well under a second), so never on
/// the UI thread. Fresh each time; models.list keeps it a minute.
pub fn check() -> Status {
    let st = match fm_path() {
        None => status_from(None, |_| None),
        Some(fm) => status_from(Some(&fm), |args| run_fm(&fm, args)),
    };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), st.clone()));
    st
}

/// The last check, when it's under a minute old; a new one otherwise.
pub fn cached() -> Status {
    if let Some((at, st)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < CACHE_FOR
    {
        return st.clone();
    }
    check()
}

static CACHE: Mutex<Option<(Instant, Status)>> = Mutex::new(None);

/// The status, given where fm is (None: nowhere) and a way to run it.
pub fn status_from(fm: Option<&Path>, run: impl Fn(&[&str]) -> Option<Ran>) -> Status {
    let no_fm = || Status {
        state: State::NoFm,
        message: "Apple's on-device model needs macOS 27 or later, which has Apple's fm tool"
            .into(),
        why: NEEDS_MACOS.into(),
        action: None,
    };
    if fm.is_none() {
        return no_fm();
    }
    let Some(lic) = run(&["license", "--status"]) else {
        return no_fm();
    };
    if !lic.ok || !lic.stdout.trim_start().starts_with("Agreed") {
        return Status {
            state: State::NoLicense,
            message: "Agree to Apple's terms for its on-device model first: run fm license in a terminal, read them and agree".into(),
            why: "Apple's terms aren't agreed yet: run fm license in a terminal".into(),
            action: None,
        };
    }
    let Some(av) = run(&["available"]) else {
        return no_fm();
    };
    if av.ok && av.stdout.trim().starts_with(AVAILABLE) {
        return Status {
            state: State::Ready,
            message: "Titles, feed summaries and quick ask now use Apple's on-device model".into(),
            why: String::new(),
            action: None,
        };
    }
    let mut reason = said(&av);
    if reason.is_empty() {
        reason = "Apple's on-device model is unavailable".into();
    }
    Status {
        state: State::Unavailable,
        message: format!(
            "Apple's on-device model can't be used yet: {}. Check that Apple Intelligence is on in System Settings › Apple Intelligence & Siri",
            reason.trim_end_matches('.')
        ),
        why: reason,
        action: cfg!(target_os = "macos").then(|| FixAction {
            label: "open System Settings".into(),
            url: SETTINGS_URL.into(),
        }),
    }
}

/// What fm said, stdout then stderr, without its "Error: ".
fn said(r: &Ran) -> String {
    let both = [r.stdout.trim(), r.stderr.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    both.strip_prefix("Error: ").unwrap_or(&both).to_string()
}

/// fm on PATH, or where macOS keeps it.
fn fm_path() -> Option<PathBuf> {
    if let Some(p) = rush_plugin::look_path("fm") {
        return Some(PathBuf::from(p));
    }
    let p = PathBuf::from("/usr/bin/fm");
    p.is_file().then_some(p)
}

fn fm_command(fm: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(fm);
    // Its help and errors are coloured otherwise.
    cmd.args(args).env("NO_COLOR", "1");
    cmd
}

fn run_fm(fm: &Path, args: &[&str]) -> Option<Ran> {
    let out = crate::exec::run(
        fm_command(fm, args),
        &Ctx::background(),
        Some(CHECK_WAIT),
        false,
    )
    .ok()?;
    Some(Ran {
        ok: out.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// The plugin's main: models.list and models.ask.
fn run(s: UnixStream) -> Result<(), String> {
    crate::serve(
        s,
        Arc::new(|_, ctx, method, params| match method {
            "initialize" => crate::empty(),
            "tools.list" => crate::no_tools(),
            "models.list" => crate::answer(&ModelList {
                models: vec![cached().model()],
            }),
            "models.ask" => {
                let q: Ask = from_raw(params).map_err(|e| Error::new(CODE_INVALID_PARAMS, e))?;
                let text = ask(ctx, &q).map_err(Error::server)?;
                crate::answer(&Answer { text })
            }
            _ => Err(no_method(method)),
        }),
    )
}

/// The model's answer to q: q.prompt on fm's stdin, q.system as its
/// instructions.
fn ask(ctx: &Ctx, q: &Ask) -> Result<String, String> {
    if q.model != MODEL {
        return Err(format!("no model named {}", q.model));
    }
    let st = cached();
    if !st.ready() {
        return Err(format!("the on-device model is unavailable: {}", st.why));
    }
    let Some(fm) = fm_path() else {
        return Err(NEEDS_MACOS.into());
    };
    let mut args = vec!["respond", "--no-stream"];
    if !q.system.is_empty() {
        args.extend(["-i", q.system.as_str()]);
    }
    let out = crate::exec::run_input(
        fm_command(&fm, &args),
        q.prompt.clone().into_bytes(),
        ctx,
        Some(ASK_WAIT),
    )
    .map_err(|_| NEEDS_MACOS.to_string())?;
    answer_from(&Ran {
        ok: out.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// What a run of fm respond answers: its text, or an error, one saying
/// "too small" for input the model can't hold.
pub fn answer_from(r: &Ran) -> Result<String, String> {
    if r.ok {
        let text = r.stdout.trim();
        if text.is_empty() {
            return Err("the on-device model gave no answer".into());
        }
        return Ok(text.to_string());
    }
    if r.stderr.contains(CONTEXT_EXCEEDED) {
        return Err(format!(
            "too small: the on-device model holds {CONTEXT_TOKENS} tokens"
        ));
    }
    let why = said(&Ran {
        stdout: String::new(),
        ..r.clone()
    });
    if why.is_empty() {
        return Err("the on-device model couldn't answer".into());
    }
    Err(why)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(ok: bool, stdout: &str, stderr: &str) -> Option<Ran> {
        Some(Ran {
            ok,
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }

    const AGREED: &str = "Agreed to license FM1 version 1.0 on 21 Oct 2026 at 19:58.\n";

    // Each check in order: the first that fails says what to fix.
    #[test]
    fn status_in_order() {
        type Run = fn(&[&str]) -> Option<Ran>;
        let cases: Vec<(&str, bool, Run, State, &str)> = vec![
            ("no fm", false, |_| None, State::NoFm, NEEDS_MACOS),
            ("fm won't start", true, |_| None, State::NoFm, NEEDS_MACOS),
            (
                "not agreed",
                true,
                |a| match a[0] {
                    "license" => ran(true, "You haven't agreed to the license.", ""),
                    _ => ran(true, "System model available", ""),
                },
                State::NoLicense,
                "Apple's terms aren't agreed yet: run fm license in a terminal",
            ),
            (
                "license fails",
                true,
                |a| match a[0] {
                    "license" => ran(false, "", "Error: something"),
                    _ => ran(true, "System model available", ""),
                },
                State::NoLicense,
                "Apple's terms aren't agreed yet: run fm license in a terminal",
            ),
            (
                "unavailable",
                true,
                |a| match a[0] {
                    "license" => ran(true, AGREED, ""),
                    _ => ran(false, "", "Error: Apple Intelligence is not enabled.\n"),
                },
                State::Unavailable,
                "Apple Intelligence is not enabled.",
            ),
            (
                "unavailable, other words",
                true,
                |a| match a[0] {
                    "license" => ran(true, AGREED, ""),
                    _ => ran(true, "Model not ready\n", ""),
                },
                State::Unavailable,
                "Model not ready",
            ),
            (
                "unavailable, nothing said",
                true,
                |a| match a[0] {
                    "license" => ran(true, AGREED, ""),
                    _ => ran(false, "", ""),
                },
                State::Unavailable,
                "Apple's on-device model is unavailable",
            ),
            (
                "ready",
                true,
                |a| match a[0] {
                    "license" => ran(true, AGREED, ""),
                    _ => ran(true, "System model available\n", ""),
                },
                State::Ready,
                "",
            ),
        ];
        for (name, has_fm, run, state, why) in cases {
            let fm = has_fm.then(|| PathBuf::from("/usr/bin/fm"));
            let st = status_from(fm.as_deref(), run);
            assert_eq!(st.state, state, "{name}");
            let m = st.model();
            assert_eq!(m.why, why, "{name}");
            assert_eq!(m.available, state == State::Ready, "{name}");
            assert_eq!(
                (m.name.as_str(), m.context_tokens),
                (MODEL, CONTEXT_TOKENS),
                "{name}"
            );
            assert_eq!(st.readiness().ready, st.ready(), "{name}");
            assert!(
                !st.message.is_empty() && !st.message.contains(".."),
                "{name}: {}",
                st.message
            );
            assert_eq!(
                st.action.is_some(),
                state == State::Unavailable && cfg!(target_os = "macos"),
                "{name}: only System Settings can fix an unavailable model"
            );
        }
    }

    #[test]
    fn answers_and_errors() {
        let cases: Vec<(&str, Ran, Result<&str, &str>)> = vec![
            (
                "answer",
                Ran {
                    ok: true,
                    stdout: "Fixing login page CSS\n".into(),
                    ..Default::default()
                },
                Ok("Fixing login page CSS"),
            ),
            (
                "too big",
                Ran {
                    ok: false,
                    stderr: "Error: The session's transcript exceeded the model's context size.\n"
                        .into(),
                    ..Default::default()
                },
                Err("too small: the on-device model holds 4096 tokens"),
            ),
            (
                "refused",
                Ran {
                    ok: false,
                    stderr: "Error: Detected content likely to be unsafe.\n".into(),
                    ..Default::default()
                },
                Err("Detected content likely to be unsafe."),
            ),
            (
                "silent failure",
                Ran::default(),
                Err("the on-device model couldn't answer"),
            ),
            (
                "empty answer",
                Ran {
                    ok: true,
                    ..Default::default()
                },
                Err("the on-device model gave no answer"),
            ),
        ];
        for (name, r, want) in cases {
            let got = answer_from(&r);
            assert_eq!(got.as_deref().map_err(String::as_str), want, "{name}");
        }
    }

    #[test]
    fn manifest_offers_models_and_is_off_until_turned_on() {
        let b = bundle();
        assert!(b.optional, "off until you turn it on");
        assert!(b.manifest.models && b.check.is_some());
        assert_eq!(b.manifest.category, "other");
    }

    // Apple's own model, through the real fm: macOS 27 with Apple
    // Intelligence on. `cargo test -p rush-bundled -- --ignored live_fm`.
    #[test]
    #[ignore = "needs macOS 27 with Apple Intelligence on"]
    fn live_fm() {
        let st = check();
        assert_eq!(st.state, State::Ready, "{st:?}");
        let q = |prompt: String| Ask {
            model: MODEL.into(),
            system: "Answer in one short line.".into(),
            prompt,
            ..Default::default()
        };
        let got = ask(&Ctx::background(), &q("Name a colour.".into()));
        assert!(got.as_ref().is_ok_and(|t| !t.is_empty()), "{got:?}");
        let big = ask(&Ctx::background(), &q("word ".repeat(9000)));
        assert!(
            big.as_ref().is_err_and(|e| e.contains("too small")),
            "{big:?}"
        );
    }
}
