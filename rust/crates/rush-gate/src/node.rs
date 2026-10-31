//! Programs a package script runs (pnpm test, nx, turbo) are found in
//! node_modules/.bin, ahead of the shims, and no hook sees them; agents
//! other than Claude have no hook at all. Every node process an agent
//! starts loads [`PRELOAD`] first, through NODE_OPTIONS, and one running a
//! gated program waits its turn there, under rush gate hold.
//! Ported from internal/gate/node.go:1-96.

use std::path::{Path, PathBuf};

use crate::rules::Rules;

/// The folder holding the preload and what it reads: which rush, which
/// names.
pub fn node_dir(root: &Path) -> PathBuf {
    root.join("node")
}

/// The file NODE_OPTIONS requires.
pub fn preload_path(root: &Path) -> PathBuf {
    node_dir(root).join("preload.cjs")
}

/// Lists rush, then the names the gate queues, one a line; gone when the
/// gate is off, and the preload does nothing.
pub fn names_path(root: &Path) -> PathBuf {
    node_dir(root).join("names")
}

/// The gated program a node script is, by the rules: the script's own name
/// (typescript/bin/tsc is tsc, vitest/vitest.mjs is vitest), else its
/// package's, else its scope's (@playwright/test is playwright); "" for
/// none.
pub fn node_name(script: &str, rules: &Rules) -> String {
    let base = base(script);
    let ext = base.rfind('.').map_or("", |i| &base[i..]);
    let base = &base[..base.len() - ext.len()];
    let mut cands = vec![base.to_string()];
    if let Some(i) = script.rfind("/node_modules/") {
        let parts: Vec<&str> = script[i + "/node_modules/".len()..].split('/').collect();
        if parts.len() > 1 && parts[0].starts_with('@') {
            cands.push(parts[1].to_string());
            cands.push(parts[0][1..].to_string());
        } else if !parts.is_empty() {
            cands.push(parts[0].to_string());
        }
    }
    cands
        .into_iter()
        .find(|c| rules.contains_key(c))
        .unwrap_or_default()
}

/// The last element of a path, as Go's filepath.Base: "." for "", "/" for
/// all slashes.
pub(crate) fn base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// The script node loads first. It costs one read of the names file in a
/// node process that runs nothing gated, and runs rush only for one that
/// might. It waits for rush gate hold to say "free" or "held", then takes
/// the held scope into its environment, so what it starts (vitest's
/// workers, a nested tsc) goes straight through. Anything amiss, and it
/// runs as without a gate.
pub const PRELOAD: &str = r#"'use strict';
(() => {
  const env = process.env;
  if (env.RUSH_GATE_HELD || !process.argv[1]) return;
  if (process.argv.slice(2).some((a) => /^(--watch|-w|--lsp|--stdio|--help|-h|--version)$/.test(a))) return;
  try { if (!require('worker_threads').isMainThread) return; } catch { return; }
  const fs = require('fs'), path = require('path');
  let lines;
  try { lines = fs.readFileSync(__dirname + '/names', 'utf8').split('\n').filter(Boolean); } catch { return; }
  const exe = lines.shift();
  let script = process.argv[1];
  try { script = fs.realpathSync(script); } catch {}
  const base = path.basename(script).replace(/\.[^.]*$/, '');
  const m = /\/node_modules\/((@[^/]+)\/)?([^/]+)\//.exec(script);
  const cands = [base];
  if (m) cands.push(m[3], ...(m[2] ? [m[2].slice(1)] : []));
  if (!exe || !cands.some((c) => lines.includes(c))) return;
  const os = require('os'), cp = require('child_process');
  const ready = path.join(os.tmpdir(), 'rush-gate-' + process.pid + '-' + Date.now());
  let child;
  try {
    child = cp.spawn(exe, ['gate', 'hold', '--ready', ready, '--pid', String(process.pid), '--dir', process.cwd(),
      '--', process.execPath, script, ...process.argv.slice(2)], { stdio: ['ignore', 'ignore', 'inherit'], detached: true });
    child.unref();
  } catch { return; }
  const nap = new Int32Array(new SharedArrayBuffer(4)), start = Date.now();
  for (;;) {
    let said;
    try { said = fs.readFileSync(ready, 'utf8'); } catch {}
    if (said !== undefined) {
      try { fs.unlinkSync(ready); } catch {}
      const got = said.split('\n').filter(Boolean);
      if (got[0] === 'held') for (const kv of got.slice(1)) { const i = kv.indexOf('='); if (i > 0) env[kv.slice(0, i)] = kv.slice(i + 1); }
      return;
    }
    // A holder that died unseen (its zombie still answers kill) stops beating.
    let beat = start;
    try { beat = fs.statSync(ready + '.beat').mtimeMs; } catch {}
    if (Date.now() - Math.max(beat, start) > 5000) return;
    Atomics.wait(nap, 0, 0, 100);
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Settings, parse_rules};

    // heavy_test.go TestNodeName
    #[test]
    fn node_names() {
        let rules = parse_rules(&Settings::new());
        for (script, want) in [
            (
                "/r/node_modules/.pnpm/typescript@7/node_modules/typescript/bin/tsc",
                "tsc",
            ),
            (
                "/r/node_modules/.pnpm/vitest@5/node_modules/vitest/vitest.mjs",
                "vitest",
            ),
            ("/r/node_modules/vitest/dist/workers/forks.js", "vitest"),
            ("/r/node_modules/@playwright/test/cli.js", "playwright"),
            ("/r/node_modules/oxlint/bin/oxlint", "oxlint"),
            ("/r/node_modules/pnpm/bin/pnpm.cjs", ""),
            ("/r/apps/api/src/main.ts", ""),
        ] {
            assert_eq!(node_name(script, &rules), want, "{script}");
        }
    }

    #[test]
    fn preload_is_gos() {
        // The preload is shared with Go rush byte for byte: the same file
        // is written whichever rush writes it.
        let go = include_str!("../testdata/node.go");
        let start = go.find("const Preload = `").unwrap() + "const Preload = `".len();
        let end = start + go[start..].find('`').unwrap();
        assert_eq!(&go[start..end], PRELOAD);
    }
}
