//! Which runs the gate queues: builds, checks and test runs, not watchers
//! or questions. Ported from internal/gate/heavy.go:1-51 and
//! workers.go:1-25.

/// Whether name run with args is the kind of run the gate queues: a build,
/// a check, a test run. A watcher, a dev server, a language server or a
/// question (--help, --version) runs for hours or a moment, and would hold
/// a slot all that time or wait for one for nothing; it goes straight
/// through.
pub fn heavy<S: AsRef<str>>(name: &str, args: &[S]) -> bool {
    for a in args {
        match a.as_ref() {
            "--watch" | "--watch=true" | "--lsp" | "--stdio" | "--help" | "-h" | "--version" => {
                return false;
            }
            "-w" if matches!(name, "tsc" | "tsgo" | "vitest" | "jest") => return false,
            _ => {}
        }
    }
    let sub = subcommand(args);
    if let Some(want) = heavy_subs(name) {
        return want.contains(&sub);
    }
    !LIGHT_SUBS.contains(&sub)
}

/// The programs that do many things, of which only these cost: go run,
/// cargo run and next dev are servers as often as not, go env and go list
/// are quick.
fn heavy_subs(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "go" => &["build", "test", "vet", "install", "generate"],
        "cargo" => &[
            "build", "test", "check", "clippy", "bench", "doc", "install",
        ],
        "vite" => &["build"],
        "next" => &["build"],
        "webpack" => &["", "build", "bundle"],
        "playwright" => &["test"],
        _ => return None,
    })
}

/// The subcommands of any other gated program that never cost.
const LIGHT_SUBS: &[&str] = &[
    "watch",
    "dev",
    "serve",
    "preview",
    "start",
    "version",
    "help",
    "completion",
    "init",
];

/// args' first word that isn't a flag, or "".
fn subcommand<S: AsRef<str>>(args: &[S]) -> &str {
    args.iter()
        .map(AsRef::as_ref)
        .find(|a| !a.is_empty() && !a.starts_with('-'))
        .unwrap_or("")
}

/// What a gated vitest run as name is told of its share of the machine: a
/// third of the cores, at least one, so runs side by side don't pile
/// workers on them and its memory stays near its estimate. A count it was
/// given stands. `given` is VITEST_MAX_WORKERS as the environment has it.
pub fn worker_env<S: AsRef<str>>(name: &str, argv: &[S], given: Option<&str>) -> Vec<String> {
    if name != "vitest" || given.is_some_and(|g| !g.is_empty()) {
        return Vec::new();
    }
    let line = argv.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" ");
    for f in [
        "--maxWorkers",
        "--max-workers",
        "--pool.maxWorkers",
        "VITEST_MAX_WORKERS",
        "--no-file-parallelism",
    ] {
        if line.contains(f) {
            return Vec::new();
        }
    }
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    vec![format!("VITEST_MAX_WORKERS={}", (cpus / 3).max(1))]
}

/// [`worker_env`] with this process's environment.
pub fn worker_env_here<S: AsRef<str>>(name: &str, argv: &[S]) -> Vec<String> {
    let given = std::env::var("VITEST_MAX_WORKERS").ok();
    worker_env(name, argv, given.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    // heavy_test.go TestHeavy
    #[test]
    fn heavy_runs() {
        for (cmd, want) in [
            ("tsc -b", true),
            ("tsc -p tsconfig.json --noEmit", true),
            ("tsc --watch", false),
            ("tsc -w", false),
            ("tsc --version", false),
            ("tsgo --lsp --stdio", false),
            ("vitest", true),
            ("vitest run src", true),
            ("vitest watch", false),
            ("vitest dev", false),
            ("go test ./...", true),
            ("go build -o x .", true),
            ("go run ./cmd/x", false),
            ("go env GOPATH", false),
            ("go list -m all", false),
            ("cargo clippy", true),
            ("cargo run", false),
            ("vite", false),
            ("vite build", true),
            ("next dev", false),
            ("next build", true),
            ("playwright test e2e", true),
            ("playwright install chromium", false),
            ("golangci-lint run ./...", true),
            ("golangci-lint version", false),
            ("oxlint --type-aware src", true),
            ("eslint --help", false),
        ] {
            let w: Vec<&str> = cmd.split_whitespace().collect();
            assert_eq!(heavy(w[0], &w[1..]), want, "Heavy({cmd:?})");
        }
    }

    // heavy_test.go TestWorkerEnv
    #[test]
    fn worker_env_shares_the_cores() {
        let env = worker_env("vitest", &["vitest", "run"], Some(""));
        assert_eq!(env.len(), 1);
        assert!(env[0].starts_with("VITEST_MAX_WORKERS="));
        assert!(worker_env("vitest", &["vitest", "--maxWorkers=1"], None).is_empty());
        assert!(worker_env("tsc", &["tsc"], None).is_empty());
        assert!(worker_env("vitest", &["vitest"], Some("2")).is_empty());
    }
}
