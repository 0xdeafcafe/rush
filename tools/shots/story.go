package main

import (
	"cmp"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Sam Rivera works at Acme on its checkout and API, and on a few things of
// their own. Everyone and everything here is made up.

const (
	loginWork     = "0a7c9d52-1111-4c6e-9d1a-5f3e2b8c4a01"
	loginPersonal = "3f2e1d0c-2222-4b5a-8c7d-6e5f4a3b2c1d"
	loginLumen    = "7b6a5948-3333-4d2c-9e1f-0a1b2c3d4e5f"
)

// hosted is a rush-mode session: its info, and for Claude Code its
// conversation.
type hosted struct {
	info host.Info
	conv *conv
	// procs are what it runs, under its host: program, arguments, MB, CPU.
	procs []procSpec
}

type procSpec struct {
	comm string
	args []string
	mb   float64
	cpu  float64
	kids []procSpec
	// cued is a process that runs only from..to after the world's cue (to
	// 0 runs on), started at start after it: a chain's commands, one after
	// another, while a shot is taken.
	cued            bool
	from, to, start time.Duration
}

// story fills the world: who Sam is, their repositories, their agents.
func story(w *world) error {
	now := w.now
	ago := func(d time.Duration) time.Time { return now.Add(-d) }
	claudeDir := filepath.Join(w.home, ".claude")
	projects := filepath.Join(claudeDir, "projects")
	src := filepath.Join(w.home, "src")
	checkout := filepath.Join(src, "acme", "checkout")
	applePay := filepath.Join(checkout, ".claude", "worktrees", "apple-pay")
	taxFix := filepath.Join(checkout, ".claude", "worktrees", "tax-rounding")
	api := filepath.Join(src, "lumen-api")
	ds := filepath.Join(src, "acme", "design-system")
	orbit := filepath.Join(src, "orbit-cli")
	notes := filepath.Join(src, "field-notes")

	if err := w.repos(ago); err != nil {
		return err
	}
	if err := w.accounts(ago); err != nil {
		return err
	}

	feat := featured(applePay, ago)
	sessions := []hosted{
		{info: host.Info{ID: feat.id[:8], Name: "add Apple Pay to checkout", Cwd: applePay, State: "working",
			Model: feat.model, Effort: "high", StartedAt: ago(62 * time.Minute), UpdatedAt: ago(3 * time.Second),
			Queue:      []string{"once the tests pass, open a draft PR against main"},
			Background: []host.Task{{ID: "t1", Type: "local_agent", Label: "Draft the rollout notes", StartedAt: ago(90 * time.Second)}}},
			conv: feat,
			procs: []procSpec{{comm: "claude", args: []string{"claude", "-p", "--output-format", "stream-json", "--model", "opus"}, mb: 486, cpu: 18, kids: []procSpec{
				{comm: "codex", args: []string{"codex", "exec", codexPrompt}, mb: 142, cpu: 9},
				{comm: "node", args: []string{"node", applePay + "/node_modules/.bin/vite", "--port", "5173"}, mb: 212, cpu: 3},
			}}}},
		{info: host.Info{ID: "a41c09e2", Name: "rate limit the public API", Cwd: api, State: "working",
			Model: "claude-sonnet-5", Effort: "medium", StartedAt: ago(26 * time.Minute), UpdatedAt: ago(8 * time.Second)},
			conv: rateLimits(api, ago),
			procs: []procSpec{{comm: "claude", args: []string{"claude", "-p", "--output-format", "stream-json"}, mb: 344, cpu: 12, kids: []procSpec{
				{comm: "zsh", args: []string{"/bin/zsh", "-c", "source " + filepath.Join(w.home, ".claude", "shell-snapshots", "snapshot-zsh.sh") + " && eval '" + checksCmd + "' < /dev/null && pwd -P >| /tmp/claude-cwd"},
					mb: 4, cpu: 0, cued: true, start: -70 * time.Second, kids: []procSpec{
						{comm: "go", args: []string{"go", "vet", "./..."}, mb: 210, cpu: 180, cued: true, to: 10 * time.Second, start: -58 * time.Second},
						{comm: "go", args: []string{"go", "test", "-race", "./..."}, mb: 96, cpu: 140, cued: true, from: 10 * time.Second, to: 17 * time.Second, start: 10 * time.Second,
							kids: []procSpec{{comm: "limit.test", args: []string{"limit.test", "-test.race"}, mb: 188, cpu: 95, cued: true, from: 10 * time.Second, to: 17 * time.Second, start: 11 * time.Second}}},
						{comm: "golangci-lint", args: []string{"golangci-lint", "run"}, mb: 540, cpu: 310, cued: true, from: 17 * time.Second, start: 17 * time.Second},
					}},
			}}}},
		{info: host.Info{ID: "c7d20b51", Kind: "codex", Account: "codex", Name: "move the design tokens to CSS variables", Cwd: ds, State: "working",
			Detail: "running pnpm build:tokens", Model: "gpt-5.5-codex", Effort: "high", CostUSD: 3.10, StartedAt: ago(18 * time.Minute), UpdatedAt: ago(5 * time.Second)},
			procs: []procSpec{{comm: "codex", args: []string{"codex", "app-server"}, mb: 118, cpu: 7, kids: []procSpec{
				{comm: "node", args: []string{"node", "scripts/build-tokens.mjs"}, mb: 164, cpu: 71},
			}}}},
		{info: host.Info{ID: "9e8f1a2b", Kind: "gemini", Account: "gemini", Name: "document the orbit CLI's flags", Cwd: orbit, State: "working",
			Detail: "writing docs/flags.md", Model: "gemini-3-pro", StartedAt: ago(11 * time.Minute), UpdatedAt: ago(12 * time.Second)},
			procs: []procSpec{{comm: "node", args: []string{"node", "gemini", "--experimental-acp"}, mb: 231, cpu: 4}}},
		{info: host.Info{ID: "b3a4c5d6", Name: "fix the flaky tax rounding test", Cwd: taxFix, State: "blocked",
			Needs: "asks: Round half up, or half to even?", Model: "claude-opus-5-5", Effort: "medium",
			StartedAt: ago(35 * time.Minute), UpdatedAt: ago(4 * time.Minute)},
			conv:  taxRounding(taxFix, ago),
			procs: []procSpec{{comm: "claude", args: []string{"claude", "-p"}, mb: 298, cpu: 0.3}}},
		{info: host.Info{ID: "d0e1f2a3", Name: "upgrade the storefront to React 19", Cwd: checkout, State: "blocked",
			Model: "claude-opus-5-5", StartedAt: ago(3 * time.Hour), UpdatedAt: ago(22 * time.Minute),
			Limit: &host.Limit{ResetsAt: limitReset(ago), Window: "five_hour", Continue: true}},
			conv:  react19(checkout, ago),
			procs: []procSpec{{comm: "claude", args: []string{"claude", "-p"}, mb: 262, cpu: 0}}},
		{info: host.Info{ID: "e5f6a7b8", Kind: "kimi", Account: "kimi", Name: "port the CSV importer to streams", Cwd: api, State: "idle",
			Error: "Connection dropped", Retry: &host.Retry{Reason: "network", GaveUp: true, Why: "Connection dropped"},
			Model: "kimi-k3", StartedAt: ago(52 * time.Minute), UpdatedAt: ago(9 * time.Minute)},
			procs: []procSpec{{comm: "kimi", args: []string{"kimi", "acp"}, mb: 88, cpu: 0}}},
		{info: host.Info{ID: "f1e2d3c4", Kind: "copilot", Account: "srivera-acme", Name: "bump the checkout's dependencies", Cwd: checkout, State: "idle",
			Detail: "18 packages updated, lockfile rewritten, tests pass", Model: "gpt-5.5", StartedAt: ago(70 * time.Minute), UpdatedAt: ago(40 * time.Second)},
			procs: []procSpec{{comm: "copilot", args: []string{"copilot", "--acp"}, mb: 154, cpu: 0.2}}},
		{info: host.Info{ID: "a9b8c7d6", Kind: "opencode", Account: "opencode", Name: "tidy the field notes README", Cwd: notes, State: "idle",
			Detail: "README reorganised under four headings", Model: "qwen3-coder", StartedAt: ago(95 * time.Minute), UpdatedAt: ago(48 * time.Minute)}},
		{info: host.Info{ID: "c1d2e3f4", Kind: "deepseek", Account: "deepseek", Name: "profile the ledger import", Cwd: api, State: "stopped",
			Detail: "the import spends 71% of its time in decimal parsing", Model: "deepseek-v4", CostUSD: 0.42, StartedAt: ago(5 * time.Hour), UpdatedAt: ago(4 * time.Hour)}},
		{info: host.Info{ID: "b5a6f7e8", Kind: "glm", Account: "glm", Name: "translate the error messages", Cwd: ds, State: "stopped",
			Detail: "added fr, de and es catalogues", Model: "glm-5", StartedAt: ago(7 * time.Hour), UpdatedAt: ago(6 * time.Hour)}},
		{info: host.Info{ID: "d9c8b7a6", Kind: "vibe", Account: "vibe", Name: "sketch a landing page for orbit", Cwd: orbit, State: "stopped",
			Detail: "a one-page site in docs/site", Model: "devstral-2", StartedAt: ago(26 * time.Hour), UpdatedAt: ago(25 * time.Hour)}},
		{info: host.Info{ID: "b3c4d5e6", Kind: "ollama-pi", Account: "ollama", Name: "write the ledger's migration notes", Cwd: api, State: "working",
			Model: "qwen3-coder:30b", StartedAt: ago(6 * time.Minute), UpdatedAt: ago(3 * time.Second)},
			conv:  migrationNotes(api, ago),
			procs: []procSpec{{comm: "pi", args: []string{"pi", "--mode", "rpc", "--provider", "ollama", "--model", "qwen3-coder:30b"}, mb: 120, cpu: 2}}},
		{info: host.Info{ID: "c7d8e9f0", Kind: "ollama-codex", Account: "ollama", Name: "explain the auth middleware", Cwd: api, State: "idle",
			Detail: "a walkthrough of the token refresh, with a diagram", Model: "gpt-oss:20b", StartedAt: ago(2 * time.Hour), UpdatedAt: ago(90 * time.Minute)},
			procs: []procSpec{{comm: "codex", args: []string{"codex", "app-server"}, mb: 64, cpu: 0}}},
		{info: host.Info{ID: "e7f8a9b0", Kind: "ollama", Account: "ollama", Name: "summarise yesterday's error logs", Cwd: notes, State: "stopped",
			Detail: "three causes, the worst a retry storm at 02:14", Model: "qwen3-coder:30b", StartedAt: ago(20 * time.Hour), UpdatedAt: ago(19 * time.Hour)}},
	}
	for _, h := range sessions {
		if err := w.hosted(h, projects); err != nil {
			return err
		}
	}
	if err := w.past(projects, checkout, api, ds, orbit, notes, ago); err != nil {
		return err
	}
	if err := w.codex(applePay, ds, ago); err != nil {
		return err
	}
	// What a session left running after it ended: a dev server nobody stopped.
	w.pid(1, "node", []string{"node", taxFix + "/node_modules/.bin/vite", "--port", "5174"}, 176, 0.4)
	return nil
}

// hosted starts one rush-mode session.
func (w *world) hosted(h hosted, projects string) error {
	info := h.info
	if info.Kind == "" {
		info.Kind, info.Account = "claude", "work"
	}
	if info.PermissionMode == "" {
		info.PermissionMode = "auto"
	}
	var replay []string
	if h.conv != nil {
		info.SessionID = h.conv.id
		r, err := h.conv.write(projects)
		if err != nil {
			return err
		}
		replay = r
	} else {
		info.SessionID = info.ID + "-6d1e-4c2b-9a8f-" + strings.Repeat(info.ID[:4], 3)
	}
	if info.State != "stopped" {
		hp, err := w.live(34, 0.6)
		if err != nil {
			return err
		}
		info.HostPID = hp
		var spawn func(ppid int, ps []procSpec)
		spawn = func(ppid int, ps []procSpec) {
			for _, p := range ps {
				pid := w.pid(ppid, p.comm, p.args, p.mb, p.cpu)
				if p.cued {
					w.cued[pid] = p
				}
				if ppid == hp && info.ClaudePID == 0 {
					info.ClaudePID = pid
				}
				spawn(pid, p.kids)
			}
		}
		spawn(hp, h.procs)
	}
	return w.session(info, replay)
}

// accounts are Sam's sign-ins: three Claude logins, two Codex, two GitHub
// accounts for Copilot, and the pay-as-you-go ones.
func (w *world) accounts(ago func(time.Duration) time.Time) error {
	claudeDir := filepath.Join(w.home, ".claude")
	logins := []claude.Login{
		{Name: "work", ID: loginWork, Email: "sam@acme.example", Org: "Acme"},
		{Name: "personal", ID: loginPersonal, Email: "sam.rivera@example.com"},
		{Name: "lumen", ID: loginLumen, Email: "sam@lumen.example", Org: "Lumen Labs"},
	}
	cfg := state.Config{
		Folders: []state.Folder{{Name: "work", ConfigDir: claudeDir}}, FoldersImported: true, Logins: logins,
		SignIns: []state.SignIn{
			{Kind: "codex", ID: "user-acme-7f3a", Name: "work", Email: "sam@acme.example", Plan: "pro"},
			{Kind: "codex", ID: "user-home-1b9c", Name: "personal", Email: "sam.rivera@example.com", Plan: "plus"},
		},
		Using:   map[string]string{"codex": "user-acme-7f3a"},
		GroupBy: "status", View: "split", EnterOn: "open", MenuBarAsked: true, BuiltinProfiles: true,
		ProfilesMovedIn: true, AtLimit: state.AtLimitNext,
		Onboarding: state.Onboarding{Hidden: true},
	}
	ov := state.Overlay{Names: map[string]string{}, Done: map[string]time.Time{}}
	for _, p := range pastConvs {
		if p.done {
			ov.Done["work/i:"+p.id[:8]] = ago(p.ago - time.Minute)
		}
	}
	// Codex's review, run from the Apple Pay session's shell, is that
	// session's: its own row is put away.
	ov.Done[".codex/i:"+codexReview[:8]] = ago(0)
	w.cfg, w.ov = cfg, ov
	if err := w.save(); err != nil {
		return err
	}
	// rtk's hook, set up the way rtk init does it.
	if err := saveJSON(filepath.Join(claudeDir, "settings.json"), map[string]any{"hooks": map[string]any{
		"PreToolUse": []any{map[string]any{"matcher": "Bash", "hooks": []any{map[string]any{"type": "command", "command": "rtk hook claude"}}}},
	}}); err != nil {
		return err
	}
	reset5h, reset7d := ago(-2*time.Hour-14*time.Minute), ago(-50*time.Hour)
	if err := saveJSON(filepath.Join(w.home, ".claude.json"), map[string]any{
		"oauthAccount": map[string]any{"accountUuid": loginWork, "emailAddress": "sam@acme.example",
			"organizationName": "Acme", "organizationRole": "admin", "organizationType": "claude_max",
			"billingType": "stripe_subscription", "userRateLimitTier": "max 20x"},
		"cachedUsageUtilization": map[string]any{"fetchedAtMs": ago(time.Minute).UnixMilli(), "utilization": map[string]any{
			"five_hour": map[string]any{"utilization": 38, "resets_at": reset5h.Format(time.RFC3339)},
			"seven_day": map[string]any{"utilization": 61, "resets_at": reset7d.Format(time.RFC3339)},
		}},
	}); err != nil {
		return err
	}
	win := func(p float64, at time.Time) claude.Window {
		return claude.Window{Present: true, Percent: p, ResetsAt: at}
	}
	readings := map[string]claude.FetchedUsage{
		"login:" + loginWork: {Usage: claude.Usage{AccountID: loginWork, Email: "sam@acme.example", Org: "Acme", Plan: "max 20x",
			FiveHour: win(38, reset5h), SevenDay: win(61, reset7d), FetchedAt: ago(time.Minute), Fetched: true}},
		"login:" + loginPersonal: {Usage: claude.Usage{AccountID: loginPersonal, Email: "sam.rivera@example.com", Plan: "max 5x",
			FiveHour: win(0, time.Time{}), SevenDay: win(22, ago(-96*time.Hour)), FetchedAt: ago(3 * time.Minute), Fetched: true}},
		"login:" + loginLumen: {Usage: claude.Usage{AccountID: loginLumen, Email: "sam@lumen.example", Org: "Lumen Labs", Plan: "team",
			FiveHour: win(91, ago(-2*time.Hour)), SevenDay: win(47, ago(-120*time.Hour)), FetchedAt: ago(2 * time.Minute), Fetched: true}},
	}
	if err := saveJSON(filepath.Join(w.home, ".config", "rush", "usage.json"), readings); err != nil {
		return err
	}
	// Codex's limits, as last read of each sign-in.
	codex := func(id, email, plan string, p5, p7 float64) usage.Quota {
		return usage.Quota{Account: "codex:" + id, Email: email, Plan: plan, FetchedAt: ago(4 * time.Minute), Source: usage.Fetched,
			Windows: []usage.Window{
				{ID: "primary", Label: "5h", Name: "5-hour", Span: 5 * time.Hour, Percent: p5, ResetsAt: ago(-3 * time.Hour)},
				{ID: "secondary", Label: "7d", Name: "weekly", Span: 7 * 24 * time.Hour, Percent: p7, ResetsAt: ago(-80 * time.Hour)},
			}}
	}
	return saveJSON(host.QuotasPath(), map[string]usage.Quota{
		"codex:user-acme-7f3a": codex("user-acme-7f3a", "sam@acme.example", "pro", 24, 52),
		"codex:user-home-1b9c": codex("user-home-1b9c", "sam.rivera@example.com", "plus", 0, 9),
	})
}

// repos are Sam's repositories: history, branches, worktrees and work not
// yet committed.
func (w *world) repos(ago func(time.Duration) time.Time) error {
	src := filepath.Join(w.home, "src")
	remotes := filepath.Join(w.root, "remotes")
	day := 24 * time.Hour
	for _, r := range []struct {
		dir     string
		files   map[string]string
		commits []string
	}{
		{filepath.Join(src, "acme", "checkout"), checkoutFiles, []string{
			"feat(cart): keep the basket across sign-in", "fix(tax): round VAT per line, not per basket",
			"chore: vitest 4", "feat(payments): one interface for every provider", "fix(checkout): the pay button waits for the total",
		}},
		{filepath.Join(src, "lumen-api"), apiFiles, []string{
			"feat: /v2/ledger", "fix(auth): refresh tokens rotate", "perf(import): stream the CSV", "chore: go 1.27",
		}},
		{filepath.Join(src, "acme", "design-system"), dsFiles, []string{"feat(button): a quiet variant", "fix(tokens): dark mode greys", "docs: colour guide"}},
		{filepath.Join(src, "orbit-cli"), orbitFiles, []string{"feat: orbit sync --dry-run", "fix: config in XDG_CONFIG_HOME", "release 0.9.0"}},
		{filepath.Join(src, "field-notes"), notesFiles, []string{"notes: incident review", "notes: on retries"}},
	} {
		for name, body := range r.files {
			if err := write(r.dir, name, body); err != nil {
				return err
			}
		}
		if err := w.git(r.dir, ago(9*day), "init", "-q", "-b", "main"); err != nil {
			return err
		}
		for i, msg := range r.commits {
			at := ago(time.Duration(len(r.commits)-i) * day * 3 / 2)
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
		if err := w.git(w.root, ago(day), "init", "-q", "--bare", bare); err != nil {
			return err
		}
		if err := w.git(r.dir, ago(day), "remote", "add", "origin", bare); err != nil {
			return err
		}
		if err := w.git(r.dir, ago(day), "push", "-q", "-u", "origin", "main"); err != nil {
			return err
		}
		// Shown as where it would really live, once pushed.
		owner := map[string]string{"checkout": "acme", "design-system": "acme", "lumen-api": "lumen-labs"}[filepath.Base(r.dir)]
		url := "git@github.com:" + cmp.Or(owner, "samrivera") + "/" + filepath.Base(r.dir) + ".git"
		if err := w.git(r.dir, ago(day), "remote", "set-url", "origin", url); err != nil {
			return err
		}
	}
	checkout := filepath.Join(src, "acme", "checkout")
	for _, wt := range []struct{ name, branch string }{{"apple-pay", "feat/apple-pay"}, {"tax-rounding", "fix/tax-rounding"}} {
		dir := filepath.Join(checkout, ".claude", "worktrees", wt.name)
		if err := w.git(checkout, ago(2*time.Hour), "worktree", "add", "-q", "-b", wt.branch, dir); err != nil {
			return err
		}
	}
	applePay := filepath.Join(checkout, ".claude", "worktrees", "apple-pay")
	if err := write(applePay, "src/payments/providers.ts", providersTS); err != nil {
		return err
	}
	if err := w.git(applePay, ago(40*time.Minute), "commit", "-q", "-am", "feat(payments): register providers by region"); err != nil {
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
	api := filepath.Join(src, "lumen-api")
	if err := w.git(api, ago(time.Hour), "checkout", "-q", "-b", "feat/rate-limits"); err != nil {
		return err
	}
	if err := write(api, "internal/limit/limit.go", limitGo); err != nil {
		return err
	}
	return write(filepath.Join(src, "acme", "design-system"), "tokens/colours.css", ":root {\n  --grey-900: #111;\n}\n")
}

// past are conversations nothing has open any more, from the last week.
type pastConv struct {
	id, dir, prompt, answer string
	ago                     time.Duration
	done                    bool
	cost                    int // roughly how many requests it made
}

var pastConvs = []pastConv{
	{"11a2b3c4-5d6e-4f70-8a9b-0c1d2e3f4a5b", "lumen-api", "why does the ledger import time out on big files?",
		"It reads the whole CSV into memory before parsing. Streaming it row by row keeps memory flat at about 40 MB and brings the 2 GB file from 14m to 2m10s.", 3 * time.Hour, false, 14},
	{"22b3c4d5-6e7f-4081-9bac-1d2e3f4a5b6c", "acme/checkout", "set up caching for CI",
		"CI now caches the pnpm store and Playwright's browsers, keyed on the lockfile. A warm run went from 11m to 4m.", 5 * time.Hour, false, 9},
	{"33c4d5e6-7f80-4192-acbd-2e3f4a5b6c7d", "acme/design-system", "rename Invoice to Bill everywhere",
		"Renamed across 41 files, kept the old export as a deprecated alias, and added a codemod for the apps that import it.", 26 * time.Hour, true, 22},
	{"44d5e6f7-8091-42a3-bdce-3f4a5b6c7d8e", "orbit-cli", "add shell completions for zsh and fish",
		"orbit completion zsh and orbit completion fish print the scripts; the README says where to put them.", 28 * time.Hour, true, 7},
	{"55e6f7a8-91a2-43b4-cedf-4a5b6c7d8e9f", "acme/checkout", "the promo code field loses focus on every keypress",
		"The field was keyed on the code itself, so React remounted it on every change. It's keyed on the form now.", 30 * time.Hour, true, 5},
	{"66f7a8b9-a2b3-44c5-dfe0-5b6c7d8e9fa0", "lumen-api", "write an ADR for moving auth to sessions",
		"docs/adr/0014-sessions.md has the decision, the two alternatives and what it costs the mobile app.", 50 * time.Hour, true, 11},
	{"77a8b9c0-b3c4-45d6-e0f1-6c7d8e9fa0b1", "field-notes", "turn my notes on retries into a post",
		"A 1,200-word draft is in posts/retries.md: backoff, jitter, budgets, and the incident that taught us each.", 52 * time.Hour, true, 6},
	{"88b9c0d1-c4d5-46e7-f102-7d8e9fa0b1c2", "acme/checkout", "why is the basket total off by a penny?",
		"Rounding per line and summing disagreed with rounding the sum. Tax is now rounded per line, as the invoice does.", 74 * time.Hour, true, 8},
	{"99c0d1e2-d5e6-47f8-0213-8e9fa0b1c2d3", "lumen-api", "add request ids to every log line",
		"Every request gets an id at the edge, and slog carries it through the handlers and into the queue workers.", 98 * time.Hour, true, 10},
	{"aad1e2f3-e6f7-4809-1324-9fa0b1c2d3e4", "orbit-cli", "make sync resumable",
		"sync now keeps a journal of what it moved; a second run after a failure picks up from the last file.", 120 * time.Hour, true, 16},
}

func (w *world) past(projects, checkout, api, ds, orbit, notes string, ago func(time.Duration) time.Time) error {
	dirs := map[string]string{"acme/checkout": checkout, "lumen-api": api, "acme/design-system": ds, "orbit-cli": orbit, "field-notes": notes}
	for i, p := range pastConvs {
		at := ago(p.ago + 20*time.Minute)
		var steps []step
		for j := range p.cost {
			steps = append(steps, step{tool: "Read", input: map[string]any{"file_path": filepath.Join(dirs[p.dir], fmt.Sprintf("src/part%d.ts", j))}, result: "…", out: 900, in: 4000})
		}
		// Most of the week ran with rtk's hook, which squeezes what the
		// shell says; the oldest sessions ran before it was set up.
		test := step{tool: "Bash", input: map[string]any{"command": "npm test", "description": "Run the tests"}, result: strings.Repeat("✓ passes a case of the suite\n", 60)}
		if i < 7 {
			test.input["command"], test.result = "rtk npm test", "tests: 60 passed (1.8s)"
		}
		steps = append(steps, test, test)
		c := conv{id: p.id, cwd: dirs[p.dir], branch: "main", model: "claude-opus-5-5",
			turns: []turn{{prompt: p.prompt, at: at, steps: steps, answer: p.answer}}}
		if _, err := c.write(projects); err != nil {
			return err
		}
	}
	return nil
}
