package gate

import (
	"path/filepath"
	"strings"
)

// Programs a package script runs (pnpm test, nx, turbo) are found in
// node_modules/.bin, ahead of the shims, and no hook sees them; agents
// other than Claude have no hook at all. Every node process an agent
// starts loads Preload first, through NODE_OPTIONS, and one running a
// gated program waits its turn there, under rush gate hold.

// NodeDir holds the preload and what it reads: which rush, which names.
func NodeDir() string { return filepath.Join(Root(), "node") }

// PreloadPath is the file NODE_OPTIONS requires.
func PreloadPath() string { return filepath.Join(NodeDir(), "preload.cjs") }

// NamesPath lists rush, then the names the gate queues, one a line; gone
// when the gate is off, and the preload does nothing.
func NamesPath() string { return filepath.Join(NodeDir(), "names") }

// NodeName is the gated program a node script is, by the rules: the
// script's own name (typescript/bin/tsc is tsc, vitest/vitest.mjs is
// vitest), else its package's, else its scope's (@playwright/test is
// playwright); "" for none.
func NodeName(script string, rules map[string]Rule) string {
	base := filepath.Base(script)
	base = strings.TrimSuffix(base, filepath.Ext(base))
	cands := []string{base}
	if i := strings.LastIndex(script, "/node_modules/"); i >= 0 {
		parts := strings.Split(script[i+len("/node_modules/"):], "/")
		if len(parts) > 1 && strings.HasPrefix(parts[0], "@") {
			cands = append(cands, parts[1], strings.TrimPrefix(parts[0], "@"))
		} else if len(parts) > 0 {
			cands = append(cands, parts[0])
		}
	}
	for _, c := range cands {
		if _, ok := rules[c]; ok {
			return c
		}
	}
	return ""
}

// Preload is the script node loads first. It costs one read of NamesPath
// in a node process that runs nothing gated, and runs rush only for one
// that might. It waits for rush gate hold to say "free" or "held", then
// takes the held scope into its environment, so what it starts (vitest's
// workers, a nested tsc) goes straight through. Anything amiss, and it
// runs as without a gate.
const Preload = `'use strict';
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
`
