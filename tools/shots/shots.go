package main

import "path/filepath"

// open goes to the featured session the way you would: by name.
var open = []string{"ctrl+k", "text=add Apple Pay to checkout", "enter"}

func view(v string) func(*world) { return func(w *world) { w.view(v) } }

// shots are the README's screenshots, each showing one thing rush does,
// at the screen that shows it.
var shots = []shot{
	// What needs you comes first: a question, beside every other agent.
	{name: "needs-you", w: 180, h: 50, prep: view("split"), keys: []string{"ctrl+k", "text=fix the flaky tax", "enter"}},
	// Every agent, of every provider, in one list.
	{name: "agents", w: 180, h: 62, prep: view("list")},
	// A failed step opened: its script and the error.
	{name: "session-code", w: 180, h: 58, prep: view("split"), keys: append(open, "up", "up", "up", "up", "up", "up", "up", "up", "up", "space")},
	// Subagents, a Codex review among them, with what each runs on.
	{name: "subagents", w: 180, h: 46, prep: view("agent"), keys: append(open, "]", "]", "]", "]")},
	// Every running agent at once.
	{name: "wall", w: 180, h: 50, keys: []string{"ctrl+k", "text=wall", "enter"}},
	// Usage switching: what each account has left, and what a profile does at a limit.
	{name: "accounts", w: 180, h: 40, keys: []string{"ctrl+k", "text=accounts", "enter"}},
	{name: "profiles", w: 180, h: 46, keys: []string{"ctrl+k", "text=settings profiles", "enter"}},
	// Whether the savers you set up are working.
	{name: "efficiency", w: 180, h: 50, keys: []string{"ctrl+k", "text=efficiency", "enter"}},
	// What rush can do with each agent, and what its models take.
	{name: "coding-agents", w: 180, h: 52, keys: []string{"ctrl+k", "text=settings agents", "enter"}},
	// Local models: Ollama's, run in Claude Code, Codex or Pi.
	{name: "local-models", w: 180, h: 52, prep: func(w *world) { w.cfg.GroupBy = "agent"; w.view("split") },
		keys: []string{"ctrl+k", "text=write the ledger's migration notes", "enter"}},
	// Building the status lines.
	{name: "statusline", w: 180, h: 52, prep: view("agent"), keys: append(open, "text=/statusline", "enter")},
	// Plugins: what's installed, and where each runs.
	{name: "plugins", w: 180, h: 40, keys: []string{"ctrl+k", "text=settings plugins", "enter"}},
	// One session as rush draws it: code highlighted, a failed test and its
	// fix, git's commit, merge and push as cards, and a chain of checks
	// running, each command timed.
	{name: "session-showcase", w: 180, h: 80, prep: func(w *world) { w.view("agent"); w.startCue() },
		keys: []string{"ctrl+k", "text=rate limit the public API", "enter", "wait=21s", "up", "up", "up", "space", "up", "up", "up", "up", "up", "up", "up", "space"}},
	// An agent's own views beside its conversation: tabs, numbers, a
	// chart and rows a click opens.
	{name: "session-views", w: 180, h: 52, prep: func(w *world) { w.view("agent"); w.agentViews() }, keys: open},
	// Every file a session changed, with its diff.
	{name: "changes", w: 180, h: 52, prep: view("agent"), keys: append(open, "]", "]", "up", "up", "space")},
	// The header's corner with a top bar of the version, the plan and the
	// machine: each provider's accounts in the order rush moves through them.
	{name: "header-accounts", w: 180, h: 16, prep: func(w *world) {
		w.view("list")
		_ = saveJSON(filepath.Join(w.home, ".config", "rush", "bars.json"), map[string]any{"top": map[string]any{
			"lines": [][]string{{"version"}, {"plan"}, {"cpu", "ram", "net", "disk", "battery"}}, "sep": " · ",
		}})
	}},
	// Searching every transcript.
	{name: "command-bar-search", w: 180, h: 46, prep: view("split"), keys: append(open, "ctrl+k", "text=merchant")},
}
