//! rush's curated list of token savers. Port of
//! internal/efficiency/catalog.go: the same savers, words, recipes and
//! cuts.

use std::fmt;
use std::sync::LazyLock;

use crate::aggregate::Metric;
use crate::estimate::{Base, Cut};

/// What a saver cuts, which says which figures should move when it's on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What tools send back.
    ToolOutput,
    /// What Claude writes.
    Terse,
    /// What the context carries: compaction, subagents.
    Context,
    /// Reading less to find things.
    Retrieval,
    /// Seeing where tokens go.
    Measure,
}

pub const KIND_NAMES: [&str; 5] = [
    "Cuts tool output",
    "Cuts what Claude writes",
    "Keeps context small",
    "Finds code with less reading",
    "Measures",
];

impl Kind {
    pub fn name(self) -> &'static str {
        KIND_NAMES[self as usize]
    }
}

/// One thing that saves tokens (or shows where they go): a tool, a
/// plugin, an MCP server or one of Claude Code's own settings.
#[derive(Clone, Debug, Default)]
pub struct Saver {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub kind: Option<Kind>,
    pub url: &'static str,
    /// What its authors say it saves, and what someone else measured,
    /// when anyone has: shown as such, never as rush's.
    pub claim: &'static str,
    pub measured: &'static str,
    /// A caution shown before installing.
    pub note: &'static str,
    /// What it can be expected to save, for the estimate against your own
    /// figures.
    pub cut: Option<Cut>,
    /// Ways to do it; the first whose program is found is used. Manual
    /// savers have none: their commands are shown to copy.
    pub install: Vec<Recipe>,
    pub remove: Vec<Recipe>,
    pub manual: Vec<&'static str>,
    /// A saver that is a Claude Code setting.
    pub setting: Option<Setting>,
    pub detect: Detect,
    /// What the scan records that's its: "hook:" a hook's command,
    /// "skill:", "cmd:" a slash command, "mcp:" a server, "bash:" a
    /// program run. Each is a prefix or, with a leading *, a substring.
    pub uses: Vec<&'static str>,
    /// The figures it should move, for the before/after panel.
    pub moves: Vec<Metric>,
}

/// One way to install or remove a saver: commands run in order, needing
/// `needs` on the PATH. A step whose skip program is already found is
/// left out.
#[derive(Clone, Debug, Default)]
pub struct Recipe {
    pub needs: &'static str,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Default)]
pub struct Step {
    pub argv: Vec<&'static str>,
    /// Leave the step out when this program is found.
    pub skip: &'static str,
}

/// A setting's value, as it's written into settings.json.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingValue {
    Int(i64),
    Str(&'static str),
    Bool(bool),
}

impl fmt::Display for SettingValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingValue::Int(n) => write!(f, "{n}"),
            SettingValue::Str(s) => f.write_str(s),
            SettingValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// A saver that's a value in settings.json: a top-level key or, with env,
/// a variable in its env block.
#[derive(Clone, Debug)]
pub struct Setting {
    pub key: &'static str,
    pub env: bool,
    pub value: SettingValue,
    /// What Claude Code does when it's unset, in words.
    pub default: &'static str,
    /// A setting that's only on at value: any other is a choice that isn't
    /// this saver.
    pub exact: bool,
}

/// How a saver is found: any of these present means installed.
#[derive(Clone, Debug, Default)]
pub struct Detect {
    /// Programs on the PATH.
    pub bins: Vec<&'static str>,
    /// Substrings of a hook's command in settings.json.
    pub hooks: Vec<&'static str>,
    /// Plugin IDs (name@marketplace) enabled.
    pub plugins: Vec<&'static str>,
    /// MCP server names, user-wide or in the plugin.
    pub mcp: Vec<&'static str>,
    /// Files under the account's folder.
    pub files: Vec<&'static str>,
    /// What "on" means when it's more than being found: "hook" (the binary
    /// alone is half set up), "plugin", "mcp".
    pub need: &'static str,
}

fn step(argv: &[&'static str]) -> Step {
    Step {
        argv: argv.to_vec(),
        skip: "",
    }
}

fn skip(argv: &[&'static str], skip: &'static str) -> Step {
    Step {
        argv: argv.to_vec(),
        skip,
    }
}

fn recipe(needs: &'static str, steps: Vec<Step>) -> Recipe {
    Recipe { needs, steps }
}

fn cut(of: Base, low: f64, high: f64, basis: &'static str) -> Option<Cut> {
    Some(Cut {
        of,
        low,
        high,
        basis,
    })
}

fn setting(
    key: &'static str,
    env: bool,
    value: SettingValue,
    default: &'static str,
    exact: bool,
) -> Option<Setting> {
    Some(Setting {
        key,
        env,
        value,
        default,
        exact,
    })
}

use Metric::*;
use SettingValue::*;

/// rush's curated list. Install commands are the projects' own documented
/// ones, checked September 2026. Cuts are from independent measurements
/// where there are any, the authors' own where not, and say which;
/// retrieval savers' per-question figures compare against reading every
/// file, so their cuts are well below them.
pub static CATALOG: LazyLock<Vec<Saver>> = LazyLock::new(|| {
    vec![
        Saver {
            id: "rtk",
            name: "rtk",
            kind: Some(Kind::ToolOutput),
            url: "https://github.com/rtk-ai/rtk",
            about: "Rust Token Killer: a hook rewrites Claude's shell commands (git, ls, tests, builds) so their output comes back compressed",
            claim: "60–90% less shell output",
            measured: "JetBrains, 86 tasks: cost flat at high effort, +7.6% at low; only a third of shell calls were in its scope. codepointer, 614M tokens of real sessions replayed: −0.5% of the bill",
            cut: cut(
                Base::Shell,
                0.0,
                0.05,
                "Measured, it barely moves the bill (−0.5% to +7.6%): the output it cuts is cached context, the cheapest tokens, and it can cost extra turns.",
            ),
            install: vec![
                recipe(
                    "brew",
                    vec![
                        skip(&["brew", "install", "rtk"], "rtk"),
                        step(&["rtk", "init", "-g", "--auto-patch"]),
                    ],
                ),
                recipe(
                    "cargo",
                    vec![
                        skip(
                            &["cargo", "install", "--git", "https://github.com/rtk-ai/rtk"],
                            "rtk",
                        ),
                        step(&["rtk", "init", "-g", "--auto-patch"]),
                    ],
                ),
            ],
            remove: vec![recipe(
                "rtk",
                vec![step(&["rtk", "init", "-g", "--uninstall"])],
            )],
            detect: Detect {
                bins: vec!["rtk"],
                hooks: vec!["rtk hook", "rtk-rewrite"],
                files: vec!["RTK.md"],
                need: "hook",
                ..Detect::default()
            },
            uses: vec!["hook:*rtk hook", "hook:*rtk-rewrite", "bash:rtk"],
            moves: vec![ToolKb, Ctx, Cost],
            ..Saver::default()
        },
        Saver {
            id: "caveman",
            name: "caveman",
            kind: Some(Kind::Terse),
            url: "https://github.com/JuliusBrussee/caveman",
            about: "a plugin that has Claude answer in terse caveman prose; code, commands and errors stay exact",
            claim: "~65% fewer output tokens",
            measured: "JetBrains, 82 tasks: −8.5% output tokens, same quality; output is a small share of most bills",
            cut: cut(
                Base::Text,
                0.0,
                0.085,
                "JetBrains measured −8.5% of output tokens against the 65% claimed.",
            ),
            install: vec![recipe(
                "claude",
                vec![
                    step(&[
                        "claude",
                        "plugin",
                        "marketplace",
                        "add",
                        "JuliusBrussee/caveman",
                    ]),
                    step(&["claude", "plugin", "install", "caveman@caveman"]),
                ],
            )],
            remove: vec![recipe(
                "claude",
                vec![step(&["claude", "plugin", "uninstall", "caveman@caveman"])],
            )],
            detect: Detect {
                plugins: vec!["caveman@caveman"],
                hooks: vec!["caveman-activate"],
                files: vec!["skills/caveman/SKILL.md"],
                ..Detect::default()
            },
            uses: vec!["hook:*caveman", "skill:caveman", "cmd:/caveman"],
            moves: vec![Out, Cost],
            ..Saver::default()
        },
        Saver {
            id: "context-mode",
            name: "context-mode",
            kind: Some(Kind::ToolOutput),
            url: "https://github.com/mksglu/context-mode",
            about: "a plugin that runs commands and reads in a sandbox, indexes the output and returns only what's asked for",
            claim: "up to 98% less tool output",
            note: "adds its own tools and hooks to every session",
            cut: cut(
                Base::ToolOut,
                0.0,
                0.15,
                "Not measured on its own; tools that shrink tool output took 0.5–2.8% off whole bills in codepointer's replay, about a sixth of the tool output they act on at most.",
            ),
            install: vec![recipe(
                "claude",
                vec![
                    step(&[
                        "claude",
                        "plugin",
                        "marketplace",
                        "add",
                        "mksglu/context-mode",
                    ]),
                    step(&["claude", "plugin", "install", "context-mode@context-mode"]),
                ],
            )],
            remove: vec![recipe(
                "claude",
                vec![step(&[
                    "claude",
                    "plugin",
                    "uninstall",
                    "context-mode@context-mode",
                ])],
            )],
            detect: Detect {
                plugins: vec!["context-mode@context-mode"],
                mcp: vec!["context-mode"],
                ..Detect::default()
            },
            uses: vec!["mcp:*context-mode", "hook:*context-mode"],
            moves: vec![ToolKb, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "serena",
            name: "Serena",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/oraios/serena",
            about: "an MCP server that reads and edits code by symbol, through language servers, instead of whole files",
            claim: "fewer, more targeted reads",
            note: "its tool definitions add to what every session starts with",
            cut: cut(
                Base::Look,
                0.0,
                0.3,
                "No saving measured end to end; code-graph tools that were (codegraph, 7 repos) cut cost 0–78% depending on the repo.",
            ),
            install: vec![recipe(
                "uv",
                vec![
                    skip(
                        &["uv", "tool", "install", "-p", "3.13", "serena-agent"],
                        "serena",
                    ),
                    step(&[
                        "claude",
                        "mcp",
                        "add",
                        "--scope",
                        "user",
                        "serena",
                        "--",
                        "serena",
                        "start-mcp-server",
                        "--context",
                        "claude-code",
                        "--project-from-cwd",
                    ]),
                ],
            )],
            remove: vec![recipe(
                "claude",
                vec![step(&[
                    "claude", "mcp", "remove", "--scope", "user", "serena",
                ])],
            )],
            detect: Detect {
                mcp: vec!["serena"],
                bins: vec!["serena"],
                need: "mcp",
                ..Detect::default()
            },
            uses: vec!["mcp:serena"],
            moves: vec![ToolKb, Start],
            ..Saver::default()
        },
        Saver {
            id: "graphify",
            name: "Graphify",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/Graphify-Labs/graphify",
            about: "builds a knowledge graph of the repo (tree-sitter, no LLM for code) and hooks steer Claude's searches and reads to it",
            claim: "71.5× fewer tokens per question, on a 52-file corpus against reading every file; ~1× on 6 files",
            measured: "KubeBlogs, 6 real feature sessions: no session-level saving; cost followed how work was split among subagents",
            note: "its full skill is about 10k tokens when invoked, and the graph goes stale without its git hooks",
            cut: cut(
                Base::Look,
                0.0,
                0.25,
                "The one test on real work found no saving; per-question figures compare against reading every file, which Claude doesn't do.",
            ),
            install: vec![recipe(
                "uv",
                vec![
                    skip(&["uv", "tool", "install", "graphifyy"], "graphify"),
                    step(&["graphify", "install"]),
                ],
            )],
            remove: vec![recipe("graphify", vec![step(&["graphify", "uninstall"])])],
            detect: Detect {
                bins: vec!["graphify"],
                hooks: vec!["graphify hook-guard", "graphify"],
                files: vec!["skills/graphify/SKILL.md"],
                need: "hook",
                ..Detect::default()
            },
            uses: vec![
                "skill:graphify",
                "cmd:/graphify",
                "bash:graphify",
                "hook:*graphify",
            ],
            moves: vec![ToolKb, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "tokensave",
            name: "tokensave",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/aovestdipaperino/tokensave",
            about: "a code-graph MCP server in Rust (tree-sitter, 50+ languages, no LLM), with hooks that steer searches to it",
            claim: "88% fewer tokens to find code, on 10 questions about its own repo",
            measured: "an independent run of its own bench: 94%; retrieval alone, no end-to-end test",
            note: "its tool schemas are ~100 KB (TOKENSAVE_TOOLS=core: ~19 KB), deferred by tool search unless a proxy turns that off; sends a token counter home unless `tokensave disable-upload-counter`",
            cut: cut(
                Base::Look,
                0.0,
                0.4,
                "No end-to-end test; code-graph MCPs that were (codegraph, 7 repos) cut cost 44% on average, 0–78% by repo.",
            ),
            install: vec![
                recipe(
                    "brew",
                    vec![
                        skip(
                            &["brew", "install", "aovestdipaperino/tap/tokensave"],
                            "tokensave",
                        ),
                        step(&["tokensave", "install", "--agent", "claude"]),
                    ],
                ),
                recipe(
                    "cargo",
                    vec![
                        skip(&["cargo", "install", "tokensave"], "tokensave"),
                        step(&["tokensave", "install", "--agent", "claude"]),
                    ],
                ),
            ],
            remove: vec![recipe("tokensave", vec![step(&["tokensave", "uninstall"])])],
            detect: Detect {
                bins: vec!["tokensave"],
                mcp: vec!["tokensave"],
                hooks: vec!["tokensave hook-pre-tool-use", "tokensave"],
                need: "mcp",
                ..Detect::default()
            },
            uses: vec!["mcp:tokensave", "hook:*tokensave"],
            moves: vec![ToolKb, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "context7",
            name: "Context7",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/upstash/context7",
            about: "current library docs on demand, instead of searching the web or reading node_modules",
            install: vec![recipe(
                "claude",
                vec![step(&[
                    "claude",
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    "--transport",
                    "http",
                    "context7",
                    "https://mcp.context7.com/mcp",
                ])],
            )],
            remove: vec![recipe(
                "claude",
                vec![step(&[
                    "claude", "mcp", "remove", "--scope", "user", "context7",
                ])],
            )],
            detect: Detect {
                mcp: vec!["context7"],
                ..Detect::default()
            },
            uses: vec!["mcp:context7", "bash:ctx7"],
            moves: vec![ToolKb],
            ..Saver::default()
        },
        Saver {
            id: "bash-output",
            name: "Shorter shell output",
            kind: Some(Kind::ToolOutput),
            about: "Claude Code keeps at most this many characters of a command's output; the rest is saved to a file it can read if it needs to",
            setting: setting("bashOutputMaxChars", false, Int(15000), "30,000", false),
            moves: vec![ToolKb, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "mcp-output",
            name: "Shorter MCP output",
            kind: Some(Kind::ToolOutput),
            about: "the most an MCP tool's answer may take up",
            setting: setting(
                "MAX_MCP_OUTPUT_TOKENS",
                true,
                Str("10000"),
                "25,000 tokens",
                false,
            ),
            moves: vec![ToolKb],
            ..Saver::default()
        },
        Saver {
            id: "autocompact",
            name: "Compact sooner",
            kind: Some(Kind::Context),
            about: "compact at 400k tokens rather than near the end of a 1M window: every request re-reads the whole context, so a fat one costs on every turn",
            setting: setting(
                "autoCompactWindow",
                false,
                Int(400000),
                "the model's window",
                false,
            ),
            cut: cut(
                Base::Fat,
                0.5,
                0.9,
                "Compacted at 400k, the context past it isn't carried; summarising and reading files again takes some of that back.",
            ),
            moves: vec![Ctx, Cost],
            ..Saver::default()
        },
        Saver {
            id: "subagent-model",
            name: "Sonnet for subagents",
            kind: Some(Kind::Context),
            about: "subagents (Explore, general-purpose) run on Sonnet unless they ask for another model",
            setting: setting(
                "CLAUDE_CODE_SUBAGENT_MODEL",
                true,
                Str("sonnet"),
                "the session's model",
                false,
            ),
            note: "a subagent that names its own model keeps it; CLAUDE_CODE_SUBAGENT_MODEL_FORCE overrides even that",
            cut: cut(
                Base::SubBig,
                0.35,
                0.6,
                "Sonnet 5 costs half of Opus 5.5 and a fifth of Fable per token; it may take more turns. No controlled study.",
            ),
            moves: vec![Cost],
            ..Saver::default()
        },
        Saver {
            id: "history",
            name: "Keep 90 days of transcripts",
            kind: Some(Kind::Measure),
            about: "Claude Code deletes transcripts after 30 days; keeping them longer lets these graphs reach further back",
            setting: setting("cleanupPeriodDays", false, Int(90), "30 days", false),
            ..Saver::default()
        },
        Saver {
            id: "ccusage",
            name: "ccusage",
            kind: Some(Kind::Measure),
            url: "https://github.com/ccusage/ccusage",
            about: "usage reports from the same transcripts, by day, week, month and session",
            manual: vec!["npx ccusage@latest daily"],
            detect: Detect {
                bins: vec!["ccusage"],
                ..Detect::default()
            },
            ..Saver::default()
        },
        Saver {
            id: "claude-monitor",
            name: "Claude Code Usage Monitor",
            kind: Some(Kind::Measure),
            url: "https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor",
            about: "a live terminal view of the 5-hour window's burn rate",
            install: vec![recipe(
                "uv",
                vec![step(&["uv", "tool", "install", "claude-monitor"])],
            )],
            remove: vec![recipe(
                "uv",
                vec![step(&["uv", "tool", "uninstall", "claude-monitor"])],
            )],
            detect: Detect {
                bins: vec!["claude-monitor"],
                ..Detect::default()
            },
            ..Saver::default()
        },
        Saver {
            id: "headroom",
            name: "headroom",
            kind: Some(Kind::ToolOutput),
            url: "https://github.com/headroomlabs-ai/headroom",
            about: "a local proxy that compresses tool output, logs and JSON before they reach the API",
            claim: "about 20% fewer tokens for coding agents",
            measured: "codepointer, 614M tokens of real sessions replayed: −2.8% of the bill",
            note: "works as a proxy through ANTHROPIC_BASE_URL: nothing shows in transcripts, and MCP tool search is turned off unless ENABLE_TOOL_SEARCH is set",
            cut: cut(
                Base::ToolOut,
                0.0,
                0.15,
                "codepointer measured −2.8% of a whole bill; one paper found tool-output cuts raising cost 6.8%.",
            ),
            manual: vec![
                r#"uv tool install --python 3.13 "headroom-ai[all]""#,
                "headroom wrap claude",
            ],
            detect: Detect {
                bins: vec!["headroom"],
                ..Detect::default()
            },
            ..Saver::default()
        },
        Saver {
            id: "claude-code-router",
            name: "claude-code-router",
            kind: Some(Kind::Context),
            url: "https://github.com/musistudio/claude-code-router",
            about: "routes Claude Code's requests to other, cheaper models",
            note: "a proxy through ANTHROPIC_BASE_URL; costs here then no longer match what you pay, and MCP tool search is off unless ENABLE_TOOL_SEARCH is set",
            manual: vec!["npm install -g @musistudio/claude-code-router", "ccr code"],
            detect: Detect {
                bins: vec!["ccr"],
                ..Detect::default()
            },
            ..Saver::default()
        },
        Saver {
            id: "codegraph",
            name: "codegraph",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/colbymchenry/codegraph",
            about: "a code-graph MCP server that Claude asks where things are and what calls what, instead of grepping and reading",
            claim: "44% lower cost, 62% fewer tokens: 7 repos, 4 runs each, with and without",
            note: "the authors found ~80% more retrieved context still held at the end of a session; about even on a Go repo",
            cut: cut(
                Base::Look,
                0.0,
                0.44,
                "Its authors' own with-and-without test: cost −44% on average, 0–78% by repo; not yet repeated by anyone else.",
            ),
            manual: vec![
                "curl -fsSL https://raw.githubusercontent.com/colbymchenry/codegraph/main/install.sh | sh",
                "codegraph install",
            ],
            detect: Detect {
                bins: vec!["codegraph"],
                mcp: vec!["codegraph"],
                need: "mcp",
                ..Detect::default()
            },
            uses: vec!["mcp:codegraph"],
            moves: vec![ToolKb, Ctx, Cost],
            ..Saver::default()
        },
        Saver {
            id: "codebase-memory",
            name: "codebase-memory-mcp",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/DeusData/codebase-memory-mcp",
            about: "a code knowledge graph as an MCP server, with skills and hooks",
            claim: "120× fewer tokens, on 5 questions",
            measured: "its paper, 31 repos: 10× fewer tokens, but 83% of answers right against 92% reading files",
            note: "cheaper answers were also worse ones in its own paper",
            cut: cut(
                Base::Look,
                0.0,
                0.5,
                "Its paper measured 10× fewer tokens for questions, at 9 points of accuracy; no whole-session test.",
            ),
            manual: vec![
                "curl -fsSL https://raw.githubusercontent.com/DeusData/codebase-memory-mcp/main/install.sh | bash",
            ],
            detect: Detect {
                mcp: vec!["codebase-memory-mcp"],
                bins: vec!["codebase-memory-mcp"],
                need: "mcp",
                ..Detect::default()
            },
            uses: vec!["mcp:codebase-memory-mcp"],
            moves: vec![ToolKb, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "lsp",
            name: "Language server plugins",
            kind: Some(Kind::Retrieval),
            url: "https://code.claude.com/docs/en/costs",
            about: "Anthropic's own plugins (gopls-lsp, typescript-lsp, pyright-lsp, rust-analyzer-lsp…) give Claude go-to-definition and references instead of grep then read",
            note: "each needs its language server on the PATH (gopls, typescript-language-server…)",
            cut: cut(
                Base::Look,
                0.0,
                0.2,
                "Recommended in Claude Code's cost guide, but not measured by anyone.",
            ),
            manual: vec![
                "/plugin install gopls-lsp@claude-plugins-official",
                "/plugin install typescript-lsp@claude-plugins-official",
            ],
            detect: Detect {
                plugins: vec![
                    "gopls-lsp@claude-plugins-official",
                    "typescript-lsp@claude-plugins-official",
                    "pyright-lsp@claude-plugins-official",
                    "rust-analyzer-lsp@claude-plugins-official",
                ],
                ..Detect::default()
            },
            moves: vec![ToolKb],
            ..Saver::default()
        },
        Saver {
            id: "claude-context",
            name: "claude-context",
            kind: Some(Kind::Retrieval),
            url: "https://github.com/zilliztech/claude-context",
            about: "semantic code search as an MCP server: the repo is embedded into a vector database",
            claim: "~40% fewer tokens at the same retrieval quality, on its own evaluation",
            note: "needs an OpenAI key (embedding costs its own tokens) and a Milvus or Zilliz database",
            cut: cut(Base::Look, 0.0, 0.4, "Its authors' own evaluation only."),
            manual: vec![
                "claude mcp add claude-context -e OPENAI_API_KEY=… -e MILVUS_ADDRESS=… -- npx @zilliz/claude-context-mcp@latest",
            ],
            detect: Detect {
                mcp: vec!["claude-context"],
                ..Detect::default()
            },
            uses: vec!["mcp:claude-context"],
            moves: vec![ToolKb],
            ..Saver::default()
        },
        Saver {
            id: "ponytail",
            name: "Ponytail",
            kind: Some(Kind::Terse),
            url: "https://github.com/DietrichGebert/ponytail",
            about: "a plugin that has Claude write less code: smaller diffs, fewer rewrites",
            claim: "−20% cost, −54% lines, on 12 tickets",
            measured: "JetBrains, 80 tasks: −10.3% cost, −15% lines, same quality",
            cut: cut(
                Base::All,
                0.05,
                0.10,
                "JetBrains measured −10.3% of cost across whole sessions (p=0.004).",
            ),
            install: vec![recipe(
                "claude",
                vec![
                    step(&[
                        "claude",
                        "plugin",
                        "marketplace",
                        "add",
                        "DietrichGebert/ponytail",
                    ]),
                    step(&["claude", "plugin", "install", "ponytail@ponytail"]),
                ],
            )],
            remove: vec![recipe(
                "claude",
                vec![step(&[
                    "claude",
                    "plugin",
                    "uninstall",
                    "ponytail@ponytail",
                ])],
            )],
            detect: Detect {
                plugins: vec!["ponytail@ponytail"],
                ..Detect::default()
            },
            uses: vec!["skill:ponytail", "hook:*ponytail"],
            moves: vec![Out, Cost],
            ..Saver::default()
        },
        Saver {
            id: "concise",
            name: "Concise output style",
            kind: Some(Kind::Terse),
            about: "Claude Code's own style that leads with the result and leaves out preamble, narration and recaps",
            setting: setting(
                "outputStyle",
                false,
                Str("Concise"),
                "the default style",
                true,
            ),
            cut: cut(
                Base::Text,
                0.0,
                0.085,
                "Not measured; the nearest, caveman, took 8.5% off output in JetBrains' test.",
            ),
            moves: vec![Out],
            ..Saver::default()
        },
        Saver {
            id: "git-instructions",
            name: "No git instructions",
            kind: Some(Kind::Context),
            about: "leaves Claude Code's commit and PR instructions and the git status snapshot out of every session's start",
            note: "Claude still runs git, but without the house rules for commits and PRs",
            setting: setting(
                "includeGitInstructions",
                false,
                Bool(false),
                "included",
                true,
            ),
            cut: cut(
                Base::Start,
                0.02,
                0.06,
                "About 1–2k tokens of what sessions start with, read again by every request.",
            ),
            moves: vec![Start],
            ..Saver::default()
        },
        Saver {
            id: "claude-mem",
            name: "claude-mem",
            kind: Some(Kind::Context),
            url: "https://github.com/thedotmack/claude-mem",
            about: "remembers past sessions and primes new ones with short summaries, fetching detail only when asked",
            note: "hooks on every prompt and tool call, and now asks you to sign in to a hosted service by default; no measured saving",
            manual: vec!["npx claude-mem install"],
            detect: Detect {
                plugins: vec!["claude-mem@thedotmack"],
                hooks: vec!["claude-mem"],
                ..Detect::default()
            },
            uses: vec!["hook:*claude-mem", "mcp:*claude-mem"],
            moves: vec![Start, Ctx],
            ..Saver::default()
        },
        Saver {
            id: "claude-hud",
            name: "claude-hud",
            kind: Some(Kind::Measure),
            url: "https://github.com/jarrodwatts/claude-hud",
            about: "a status line showing how full the context is, as you work",
            manual: vec![
                "/plugin marketplace add jarrodwatts/claude-hud",
                "/plugin install claude-hud",
            ],
            detect: Detect {
                hooks: vec!["claude-hud"],
                ..Detect::default()
            },
            ..Saver::default()
        },
    ]
});

/// The catalog's saver with this id.
pub fn find(id: &str) -> Option<&'static Saver> {
    CATALOG.iter().find(|s| s.id == id)
}

impl Saver {
    /// Whether a use recorded in a transcript is this saver's.
    pub fn matches(&self, key: &str) -> bool {
        let (k, v) = key.split_once(':').unwrap_or((key, ""));
        for u in &self.uses {
            let (kind, pat) = u.split_once(':').unwrap_or((u, ""));
            if kind != k {
                continue;
            }
            if let Some(sub) = pat.strip_prefix('*') {
                if v.contains(sub) {
                    return true;
                }
            } else if v == pat
                || v.strip_prefix(pat)
                    .is_some_and(|r| r.starts_with(':') || r.starts_with(' '))
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches() {
        let (rtk, cave, serena) = (
            find("rtk").unwrap(),
            find("caveman").unwrap(),
            find("serena").unwrap(),
        );
        for (s, key, want) in [
            (rtk, "bash:rtk", true),
            (rtk, "bash:rtk/admin", false),
            (rtk, "hook:/opt/homebrew/bin/rtk hook claude", true),
            (rtk, "hook:node other.js", false),
            (cave, "skill:caveman:caveman", true),
            (cave, "skill:caveman", true),
            (cave, "cmd:/caveman", true),
            (cave, "cmd:/cavemanx", false),
            (cave, "hook:node x/caveman-activate.js", true),
            (serena, "mcp:serena", true),
            (serena, "mcp:serenade", false),
            (
                find("context-mode").unwrap(),
                "mcp:plugin_context-mode_context-mode",
                true,
            ),
        ] {
            assert_eq!(s.matches(key), want, "{} matches {key:?}", s.id);
        }
    }

    #[test]
    fn every_saver_once() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|s| s.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert_eq!(n, 25);
    }
}
