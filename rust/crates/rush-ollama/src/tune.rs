//! How Claude Code runs on a local model: pointed at Ollama, fitted to the
//! model's window, with lean tools; and which model to run.
//!
//! Ported from Go's internal/adapters/ollama/tune.go:1-182.

use std::time::Duration;

use rush_core::adapter::Result;

use crate::api::Model;
use crate::{Local, err, lock};

/// The model to pull when there's none that can do the work: a mixture of
/// experts with 3B active, quick on a laptop, and trained for agents that
/// code.
pub const SUGGEST: &str = "qwen3.6:35b-a3b-coding";

/// The only tools a local model is given. Each tool Claude Code offers is
/// more of the prompt a local model reads before it answers, and more ways
/// to go wrong: with these alone Claude Code's prompt is about 4k tokens
/// rather than 14k, and the first answer comes in seconds rather than a
/// minute.
pub const LEAN_TOOLS: &str = "Bash,Read,Edit,Write,Glob,Grep";

/// The models Claude Code names for itself: its titles, subagents and
/// summaries. Each goes to the one model, so a session never loads a
/// second one beside it, nor reaches Anthropic.
pub const ALIASES: [&str; 7] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
];

/// How Claude Code runs on model m, served at base: the environment that
/// points it at Ollama and fits it to the model, and its flags, with the
/// lean ones added unless flags already say otherwise.
pub fn tune(m: &Model, base: &str, flags: &[String]) -> (Vec<String>, Vec<String>) {
    let mut env = vec![
        format!("ANTHROPIC_BASE_URL={base}"),
        "ANTHROPIC_AUTH_TOKEN=ollama".to_string(),
        "ANTHROPIC_API_KEY=".to_string(), // never the user's own key, sent to Ollama
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1".to_string(),
    ];
    env.extend(ALIASES.iter().map(|a| format!("{a}={}", m.name)));
    if !m.can("thinking") {
        env.push("CLAUDE_CODE_DISABLE_THINKING=1".into());
    }
    // Claude Code takes a model it doesn't know for 200k tokens, and
    // compacts late for one loaded with less: tell it the real window.
    let n = window(m);
    if n > 0 {
        env.push(format!("CLAUDE_CODE_AUTO_COMPACT_WINDOW={n}"));
        env.push(format!(
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS={}",
            32000.min(n / 4)
        ));
    }
    let mut out = flags.to_vec();
    if !has(&out, "--tools") {
        let mut tools = LEAN_TOOLS.to_string();
        if skilled(m) {
            tools.push_str(",Skill");
        }
        out.push("--tools".into());
        out.push(tools);
    }
    // The date, git status and the like go after the prompt rather than in
    // it, so Ollama's cache of the prompt holds from one session to the
    // next.
    if !has(&out, "--exclude-dynamic-system-prompt-sections") {
        out.push("--exclude-dynamic-system-prompt-sections".into());
    }
    (env, out)
}

/// The context a model needs to be given skills: their list is thousands
/// of tokens more to read before every answer.
// ponytail: one threshold; a per-model setting if it's wrong for one.
const SKILL_WINDOW: i64 = 128 * 1024;

/// Whether m is big enough to be given Claude Code's skills.
pub fn skilled(m: &Model) -> bool {
    window(m) >= SKILL_WINDOW
}

/// The context m has: what the server loaded it with, else what it was
/// trained to; 0 when neither is known.
pub fn window(m: &Model) -> i64 {
    if m.context > 0 {
        m.context
    } else {
        m.max_context
    }
}

fn has(flags: &[String], f: &str) -> bool {
    flags
        .iter()
        .any(|x| x == f || x.strip_prefix(f).is_some_and(|r| r.starts_with('=')))
}

/// How good a first choice m is: one in memory already costs nothing to
/// start, and one made for code does the work better.
pub fn rank(m: &Model) -> i32 {
    let mut r = 0;
    if m.vram > 0 {
        r += 4;
    }
    if m.name.contains("cod") {
        r += 2;
    }
    if !m.can("thinking") || m.name.contains("instruct") {
        r += 1; // answers without a long think first
    }
    r
}

/// How long a model is given to load.
const LOAD_FOR: Duration = Duration::from_secs(5 * 60);
/// How long choosing a model is given, each ask.
const CHOOSE_FOR: Duration = Duration::from_secs(20);

impl Local {
    /// The model to run: want when it's named, else one already in memory,
    /// else the likeliest coder the server has. It must call tools, which
    /// Claude Code does everything with. It's loaded before it returns, so
    /// what it was loaded with is known and the first turn doesn't wait.
    pub fn pick(&self, want: &str) -> Result<Model> {
        self.ensure_running()?;
        let info = self.choose(want)?;
        if info.can("decision") {
            return Err(err(format!(
                "{} is a decision classifier, not a coding model",
                info.name
            )));
        }
        if !info.can_code() {
            return Err(err(format!(
                "{} can't call tools, and a coding agent does all its work with them: try `ollama pull {SUGGEST}`",
                info.name
            )));
        }
        let mut info = self.configured_model(info, Some(LOAD_FOR))?;
        self.load(&info.name, Some(LOAD_FOR))?;
        if let Ok(in_mem) = self.loaded(Some(CHOOSE_FOR))
            && let Some(l) = in_mem.get(&info.name)
        {
            info.context = l.context;
            info.vram = l.vram;
        }
        lock(&self.known).insert(info.name.clone(), info.clone());
        Ok(info)
    }

    fn choose(&self, want: &str) -> Result<Model> {
        let t = Some(CHOOSE_FOR);
        if !want.is_empty() {
            return self
                .show(want, t)
                .map_err(|e| err(format!("{e}: pull it with `ollama pull {want}`")));
        }
        let in_mem = self.loaded(t)?;
        let mut models = Vec::new();
        for n in self.names(t)? {
            if n.starts_with("rush-context-") {
                continue;
            }
            if let Ok(mut m) = self.show(&n, t)
                && m.can_code()
            {
                m.vram = in_mem.get(&n).map_or(0, |l| l.vram);
                models.push(m);
            }
        }
        models.sort_by_key(|m| std::cmp::Reverse(rank(m)));
        models.into_iter().next().ok_or_else(|| {
            err(format!(
                "Ollama has no model that can call tools: try `ollama pull {SUGGEST}`"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::serve;
    use rush_core::adapter::Registry;
    use std::collections::HashMap;

    fn env_of(env: &[String]) -> HashMap<String, String> {
        env.iter()
            .map(|e| {
                let (k, v) = e.split_once('=').unwrap_or((e, ""));
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    #[test]
    fn sends_everything_to_the_one_model() {
        let m = Model {
            context: 65536,
            ..Model::named("qwen3.6:35b-a3b-coding", &["tools", "thinking"])
        };
        let (env, flags) = tune(&m, "http://127.0.0.1:11434", &[]);
        let e = env_of(&env);
        for a in ALIASES {
            assert_eq!(e[a], m.name, "{a}");
        }
        assert_eq!(e["ANTHROPIC_BASE_URL"], "http://127.0.0.1:11434");
        assert_eq!(e["ANTHROPIC_API_KEY"], "");
        assert_eq!(e["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "65536");
        assert_eq!(e["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "16384");
        assert!(!e.contains_key("CLAUDE_CODE_DISABLE_THINKING"));
        assert!(flags.iter().any(|f| f == LEAN_TOOLS));
        assert!(
            flags
                .iter()
                .any(|f| f == "--exclude-dynamic-system-prompt-sections")
        );
    }

    #[test]
    fn fits_the_model() {
        let m = Model {
            max_context: 262144,
            ..Model::named("qwen3-coder:30b", &["tools"])
        };
        let (env, flags) = tune(&m, "http://h:1", &["--tools".into(), "default".into()]);
        let e = env_of(&env);
        assert_eq!(e["CLAUDE_CODE_DISABLE_THINKING"], "1");
        assert_eq!(e["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "262144");
        assert_eq!(e["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "32000");
        assert!(
            !flags.iter().any(|f| f == LEAN_TOOLS),
            "tools asked for were replaced"
        );
        // Big enough for skills: the Skill tool is given too.
        let (_, flags) = tune(&m, "http://h:1", &[]);
        assert!(
            flags
                .iter()
                .any(|f| f == "Bash,Read,Edit,Write,Glob,Grep,Skill")
        );
        assert!(has(&["--tools=x".into()], "--tools") && !has(&["--toolsy".into()], "--tools"));
    }

    #[test]
    fn rank_prefers_loaded_then_coders() {
        let mut vl = Model::named("qwen3-vl:30b", &["tools", "thinking", "vision"]);
        let coder = Model::named("qwen3.6:35b-a3b-coding", &["tools", "thinking"]);
        assert!(
            rank(&coder) > rank(&vl),
            "a coder should come before a vision model"
        );
        vl.vram = 1;
        assert!(
            rank(&vl) > rank(&coder),
            "a model in memory should come first"
        );
    }

    #[test]
    fn a_decision_model_cant_run_a_coding_harness() {
        let classifier = Model::named("tev1:4b", &["decision", "tools", "completion"]);
        assert!(!classifier.can_code());
        assert!(Model::named("", &["tools", "completion"]).can_code());
        let url = serve(|r| match r.path.as_str() {
            "/api/version" => (200, "{}".into()),
            "/api/show" => (
                200,
                r#"{"capabilities":["decision","tools","completion"]}"#.into(),
            ),
            p => panic!("must refuse before loading: {p}"),
        });
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(Registry::new(), Some(&url), Some(d.path().into()), None);
        let e = l.pick("tev1:4b").unwrap_err();
        assert!(e.to_string().contains("decision classifier"), "{e}");
    }

    // With none named, the likeliest coder is picked and loaded, and what
    // it was loaded with read back.
    #[test]
    fn picks_and_loads_the_likeliest_coder() {
        let url = serve(|r| {
            let out = match r.path.as_str() {
                "/api/version" | "/api/generate" => "{}",
                "/api/tags" => {
                    r#"{"models":[{"name":"chat"},{"name":"qwen-coder"},{"name":"embed"}]}"#
                }
                "/api/ps" => {
                    r#"{"models":[{"name":"qwen-coder","context_length":8192,"size_vram":5}]}"#
                }
                "/api/show" if r.json()["model"] == "embed" => r#"{"capabilities":["embedding"]}"#,
                "/api/show" => r#"{"capabilities":["tools","thinking"]}"#,
                p => panic!("{p}"),
            };
            (200, out.to_string())
        });
        let d = tempfile::tempdir().unwrap();
        let l = Local::with(Registry::new(), Some(&url), Some(d.path().into()), None);
        let m = l.pick("").unwrap();
        assert_eq!(
            (m.name.as_str(), m.context, m.vram),
            ("qwen-coder", 8192, 5)
        );
        assert_eq!(l.cached_window("qwen-coder"), 8192);
    }
}
