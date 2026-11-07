package main

import (
	"cmp"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// world is Sam's machine, made up: a home with repositories, Claude Code's
// transcripts of the agents that worked in them, and rush's settings, all
// under one folder that belongs to nobody.
type world struct {
	root         string // everything is under here
	home         string // $HOME
	bin          string // the fake programs, first on PATH
	now          time.Time
	fixtures     []*exec.Cmd
	fixtureInput []io.WriteCloser
}

func (w *world) ago(d time.Duration) time.Time { return w.now.Add(-d) }

// build makes the world afresh in root.
func build(root string) (*world, error) {
	// rush's helpers can take a moment to go once it's stopped.
	var err error
	for range 20 {
		if err = os.RemoveAll(root); err == nil {
			break
		}
		time.Sleep(250 * time.Millisecond)
	}
	if err != nil {
		return nil, err
	}
	w := &world{root: root, home: filepath.Join(root, "home"), bin: filepath.Join(root, "bin"), now: time.Now()}
	for _, d := range []string{w.bin, filepath.Join(root, "tmp"), filepath.Join(w.home, ".config", "rush"),
		filepath.Join(w.home, ".cache"), filepath.Join(w.home, ".local", "share"), filepath.Join(w.home, ".codex")} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return nil, err
		}
	}
	steps := []func() error{w.repos, w.convs, w.bins, w.config}
	for _, s := range steps {
		if err := s(); err != nil {
			return nil, err
		}
	}
	return w, nil
}

// env is everything rush is started with: nothing of the real home.
func (w *world) env() []string {
	return []string{
		"HOME=" + w.home,
		"RUSH_HOME=" + filepath.Join(w.home, ".config", "rush"),
		"XDG_CONFIG_HOME=" + filepath.Join(w.home, ".config"),
		"XDG_CACHE_HOME=" + filepath.Join(w.home, ".cache"),
		"XDG_DATA_HOME=" + filepath.Join(w.home, ".local", "share"),
		"CLAUDE_CONFIG_DIR=" + filepath.Join(w.home, ".claude"),
		"CODEX_HOME=" + filepath.Join(w.home, ".codex"),
		"TMPDIR=" + filepath.Join(w.root, "tmp"),
		"PATH=" + w.bin + ":/usr/bin:/bin:/usr/sbin:/sbin",
		"SHELL=/bin/zsh",
		"TERM=xterm-256color",
		"COLORTERM=truecolor",
		"LANG=en_GB.UTF-8",
		"USER=sam",
	}
}

// script writes the program tmux runs: rush, in the world's environment
// and nothing else.
func (w *world) script(rush string) (string, error) {
	var b strings.Builder
	b.WriteString("#!/bin/sh\nexec env -i")
	for _, e := range w.env() {
		fmt.Fprintf(&b, " %q", e)
	}
	fmt.Fprintf(&b, " %q\n", rush)
	p := filepath.Join(w.root, "run.sh")
	return p, os.WriteFile(p, []byte(b.String()), 0o755)
}

func (w *world) git(dir string, at time.Time, args ...string) error {
	stamp := at.Format(time.RFC3339)
	cmd := exec.Command("git", append([]string{"-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
		"-c", "user.name=Sam Rivera", "-c", "user.email=sam@example.com", "-c", "init.defaultBranch=main"}, args...)...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_DATE="+stamp, "GIT_COMMITTER_DATE="+stamp, "GIT_CONFIG_GLOBAL=/dev/null")
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return nil
}

func write(dir, name, body string) error {
	p := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	return os.WriteFile(p, []byte(body), 0o644)
}

func (w *world) src(p ...string) string {
	return filepath.Join(append([]string{w.home, "src"}, p...)...)
}

// repos are Sam's repositories: history, branches, worktrees and work not
// yet committed.
func (w *world) repos() error {
	remotes := filepath.Join(w.root, "remotes")
	day := 24 * time.Hour
	for _, r := range []struct {
		dir     string
		files   map[string]string
		commits []string
	}{
		{w.src("acme", "checkout"), checkoutFiles, []string{
			"feat(cart): keep the basket across sign-in", "fix(tax): round VAT per line, not per basket",
			"chore: vitest 4", "feat(payments): one interface for every provider", "fix(checkout): the pay button waits for the total",
		}},
		{w.src("lumen-api"), apiFiles, []string{
			"feat: /v2/ledger", "fix(auth): refresh tokens rotate", "perf(import): stream the CSV", "chore: go 1.27",
		}},
		{w.src("acme", "design-system"), dsFiles, []string{"feat(button): a quiet variant", "fix(tokens): dark mode greys", "docs: colour guide"}},
		{w.src("orbit-cli"), orbitFiles, []string{"feat: orbit sync --dry-run", "fix: config in XDG_CONFIG_HOME", "release 0.9.0"}},
		{w.src("field-notes"), notesFiles, []string{"notes: incident review", "notes: on retries"}},
	} {
		for name, body := range r.files {
			if err := write(r.dir, name, body); err != nil {
				return err
			}
		}
		if err := w.git(r.dir, w.ago(9*day), "init", "-q", "-b", "main"); err != nil {
			return err
		}
		for i, msg := range r.commits {
			at := w.ago(time.Duration(len(r.commits)-i) * day * 3 / 2)
			if err := write(r.dir, "CHANGELOG.md", strings.Join(r.commits[:i+1], "\n")+"\n"); err != nil {
				return err
			}
			if err := w.git(r.dir, at, "add", "-A"); err != nil {
				return err
			}
			if err := w.git(r.dir, at, "commit", "-q", "-m", msg); err != nil {
				return err
			}
		}
		bare := filepath.Join(remotes, filepath.Base(r.dir)+".git")
		if err := os.MkdirAll(remotes, 0o755); err != nil {
			return err
		}
		if err := w.git(remotes, w.ago(day), "init", "-q", "--bare", bare); err != nil {
			return err
		}
		if err := w.git(r.dir, w.ago(day), "remote", "add", "origin", bare); err != nil {
			return err
		}
		if err := w.git(r.dir, w.ago(day), "push", "-q", "-u", "origin", "main"); err != nil {
			return err
		}
		// Shown as where it would really live, once pushed.
		owner := map[string]string{"checkout": "acme", "design-system": "acme", "lumen-api": "lumen-labs"}[filepath.Base(r.dir)]
		url := "git@github.com:" + cmp.Or(owner, "samrivera") + "/" + filepath.Base(r.dir) + ".git"
		if err := w.git(r.dir, w.ago(day), "remote", "set-url", "origin", url); err != nil {
			return err
		}
	}
	checkout := w.src("acme", "checkout")
	for _, wt := range []struct{ name, branch string }{{"apple-pay", "feat/apple-pay"}, {"tax-rounding", "fix/tax-rounding"}} {
		dir := filepath.Join(checkout, ".claude", "worktrees", wt.name)
		if err := w.git(checkout, w.ago(2*time.Hour), "worktree", "add", "-q", "-b", wt.branch, dir); err != nil {
			return err
		}
	}
	applePay := filepath.Join(checkout, ".claude", "worktrees", "apple-pay")
	if err := write(applePay, "src/payments/providers.ts", providersTS); err != nil {
		return err
	}
	if err := w.git(applePay, w.ago(40*time.Minute), "commit", "-q", "-am", "feat(payments): register providers by region"); err != nil {
		return err
	}
	// The session's work so far, not yet committed.
	for name, body := range map[string]string{
		"src/payments/apple-pay.ts":     applePayTS,
		"src/checkout/CheckoutPage.tsx": checkoutPageNew,
		"config/payments.json":          paymentsJSONNew,
	} {
		if err := write(applePay, name, body); err != nil {
			return err
		}
	}
	api := w.src("lumen-api")
	if err := w.git(api, w.ago(time.Hour), "checkout", "-q", "-b", "feat/rate-limits"); err != nil {
		return err
	}
	if err := write(api, "internal/limit/limit.go", limitGo); err != nil {
		return err
	}
	return write(w.src("acme", "design-system"), "tokens/colours.css", ":root {\n  --grey-900: #111;\n}\n")
}

// convs writes every conversation Claude Code would have kept.
func (w *world) convs() error {
	projects := filepath.Join(w.home, ".claude", "projects")
	checkout := w.src("acme", "checkout")
	for _, c := range []*conv{
		featured(filepath.Join(checkout, ".claude", "worktrees", "apple-pay"), w.ago),
		rateLimits(w.src("lumen-api"), w.ago),
		taxRounding(filepath.Join(checkout, ".claude", "worktrees", "tax-rounding"), w.ago),
		react19(w.src("acme", "design-system"), w.ago),
		deploy(checkout, w.ago),
	} {
		if err := c.write(projects); err != nil {
			return err
		}
	}
	dirs := map[string]string{"acme/checkout": checkout, "lumen-api": w.src("lumen-api"), "acme/design-system": w.src("acme", "design-system"),
		"orbit-cli": w.src("orbit-cli"), "field-notes": w.src("field-notes")}
	for _, p := range pastConvs {
		at := w.ago(p.ago + 20*time.Minute)
		var steps []step
		for j := range p.cost {
			steps = append(steps, step{tool: "Read", input: map[string]any{"file_path": filepath.Join(dirs[p.dir], fmt.Sprintf("src/part%d.ts", j))}, result: "…", out: 900, in: 4000})
		}
		steps = append(steps, step{tool: "Bash", input: map[string]any{"command": "npm test", "description": "Run the tests"}, result: "tests: 60 passed (1.8s)"})
		c := conv{id: p.id, cwd: dirs[p.dir], branch: "main", model: "claude-opus-5-5",
			turns: []turn{{prompt: p.prompt, at: at, steps: steps, answer: p.answer}}}
		if err := c.write(projects); err != nil {
			return err
		}
	}
	return nil
}

// past are conversations nothing has open any more, from the last week.
type pastConv struct {
	id, dir, prompt, answer string
	ago                     time.Duration
	cost                    int // roughly how many requests it made
}

var pastConvs = []pastConv{
	{"11a2b3c4-5d6e-4f70-8a9b-0c1d2e3f4a5b", "lumen-api", "why does the ledger import time out on big files?",
		"It reads the whole CSV into memory before parsing. Streaming it row by row keeps memory flat at about 40 MB and brings the 2 GB file from 14m to 2m10s.", 3 * time.Hour, 14},
	{"22b3c4d5-6e7f-4081-9bac-1d2e3f4a5b6c", "acme/checkout", "set up caching for CI",
		"CI now caches the pnpm store and Playwright's browsers, keyed on the lockfile. A warm run went from 11m to 4m.", 5 * time.Hour, 9},
	{"33c4d5e6-7f80-4192-acbd-2e3f4a5b6c7d", "acme/design-system", "rename Invoice to Bill everywhere",
		"Renamed across 41 files, kept the old export as a deprecated alias, and added a codemod for the apps that import it.", 26 * time.Hour, 22},
	{"44d5e6f7-8091-42a3-bdce-3f4a5b6c7d8e", "orbit-cli", "add shell completions for zsh and fish",
		"orbit completion zsh and orbit completion fish print the scripts; the README says where to put them.", 28 * time.Hour, 7},
	{"55e6f7a8-91a2-43b4-cedf-4a5b6c7d8e9f", "acme/checkout", "the promo code field loses focus on every keypress",
		"The field was keyed on the code itself, so React remounted it on every change. It's keyed on the form now.", 30 * time.Hour, 5},
	{"66f7a8b9-a2b3-44c5-dfe0-5b6c7d8e9fa0", "lumen-api", "write an ADR for moving auth to sessions",
		"docs/adr/0014-sessions.md has the decision, the two alternatives and what it costs the mobile app.", 50 * time.Hour, 11},
	{"77a8b9c0-b3c4-45d6-e0f1-6c7d8e9fa0b1", "field-notes", "turn my notes on retries into a post",
		"A 1,200-word draft is in posts/retries.md: backoff, jitter, budgets, and the incident that taught us each.", 52 * time.Hour, 6},
}

// bins are the programs a shell box runs: they print what the real ones
// would, slowly enough to catch a run in the middle. claude is the real
// one, so rush lists Claude Code's sessions; it's never run.
func (w *world) bins() error {
	progs := map[string]string{
		"pnpm": `#!/bin/sh
case "$1" in
install) echo "Lockfile is up to date, resolution step is skipped"; sleep 0.6
  echo "Packages: +1284"; sleep 0.8; echo "Progress: resolved 1284, reused 1284, downloaded 0, added 1284, done"; sleep 0.7
  echo "Done in 2.4s";;
db:migrate) echo "> @acme/checkout db:migrate"; sleep 0.4; echo "connecting to staging (eu-west)"; sleep 0.6
  echo "applied 0042_checkout_sessions.sql (212ms)"; echo "1 migration applied";;
build) echo "> @acme/checkout build"; echo "vite v7.1.3 building for production..."; sleep 1
  for f in index checkout cart account vendor; do sleep 0.5; echo "dist/assets/$f-4f1c9a2e.js"; done
  echo "built in 3.12s";;
esac
`,
		"fly": `#!/bin/sh
echo "==> Verifying app config"; sleep 0.5; echo "--> Verified app config"
echo "==> Building image with Depot"; sleep 1
for s in "[1/6] FROM node:24-alpine" "[2/6] WORKDIR /app" "[3/6] COPY package.json pnpm-lock.yaml ./" "[4/6] RUN pnpm install --prod" "[5/6] COPY dist ./dist" "[6/6] exporting to image"; do echo " => $s"; sleep 0.9; done
echo "--> Pushing image done: registry.fly.io/checkout-staging:deployment-01K9XQ"
echo "==> Rolling deploy (2 machines)"
for m in 1 2; do sleep 2; echo "  Updating machine 6e82$m (ams) [1/2 health checks passing]"; sleep 2; echo "  Machine 6e82$m (ams) update succeeded"; done
echo "Visit your newly deployed app at https://checkout-staging.acme.example/"
`,
		"curl": "#!/bin/sh\nsleep 0.3; echo 'ok build=4.2.0+9f3c2e1'\n",
	}
	for name, body := range progs {
		if err := os.WriteFile(filepath.Join(w.bin, name), []byte(body), 0o755); err != nil {
			return err
		}
	}
	claude, err := exec.LookPath("claude")
	if err != nil {
		return fmt.Errorf("claude isn't installed, and rush lists Claude Code's sessions only when it is: %w", err)
	}
	if real, err := filepath.EvalSymlinks(claude); err == nil {
		claude = real
	}
	if err := os.Symlink(claude, filepath.Join(w.bin, "claude")); err != nil {
		return err
	}
	// The shell box runs $SHELL -lc, whose login profile puts the system's
	// folders first: this puts the fake programs back in front.
	if err := write(w.home, ".zprofile", fmt.Sprintf("export PATH=%q:$PATH\n", w.bin)); err != nil {
		return err
	}
	return write(w.home, ".zshrc", "")
}

// config answers rush's first-run questions (layout, menu bar, plugins,
// getting started) so they don't cover the screen.
func (w *world) config() error {
	return write(filepath.Join(w.home, ".config", "rush"), "config.json", `{
  "foldersImported": true,
  "defaultProfile": "claude",
  "profilesMovedIn": true,
  "builtinProfiles": true,
  "dispatch": {"kind": "claude", "harnessStarts": true},
  "view": "split",
  "menuBarAsked": true,
  "onboarding": {
    "steps": ["session", "hash"],
    "plugins": ["ts", "drive", "watch", "cleanup", "gate", "git-clean", "hex", "remote", "remote-ssh",
      "remote-tailscale", "apple-intelligence", "search-guard"],
    "hidden": true
  }
}
`)
}
