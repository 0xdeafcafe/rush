package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"

	_ "github.com/0xdeafcafe/rush/internal/adapters/claude"
	_ "github.com/0xdeafcafe/rush/internal/adapters/codex"
	_ "github.com/0xdeafcafe/rush/internal/adapters/copilot"
)

func TestSpawnOf(t *testing.T) {
	for _, c := range []struct {
		cmd                     string
		ok                      bool
		kind, prompt, from, dir string
		model                   string
	}{
		{cmd: `claude -p "fix the flaky test" --model sonnet`, ok: true, kind: "claude", prompt: "fix the flaky test", model: "sonnet"},
		{cmd: `claude --print --output-format json 'say hi'`, ok: true, kind: "claude", prompt: "say hi"},
		{cmd: `cd /work/app && timeout 600 claude -p "review it"`, ok: true, kind: "claude", prompt: "review it", dir: "/work/app"},
		{cmd: "claude -p \"$(cat <<'EOF'\nread the spec\nand plan it\nEOF\n)\"", ok: true, kind: "claude", prompt: "read the spec\nand plan it"},
		{cmd: `echo "summarise the diff" | claude -p`, ok: true, kind: "claude", prompt: "summarise the diff"},
		{cmd: `cat prompt.md | claude -p`, ok: true, kind: "claude", from: "prompt.md"},
		{cmd: `claude -p < prompt.md`, ok: true, kind: "claude", from: "prompt.md"},
		{cmd: `codex exec -m gpt-5 -C /work/x "port it to rust"`, ok: true, kind: "codex", prompt: "port it to rust", model: "gpt-5", dir: "/work/x"},
		{cmd: `codex exec --full-auto "tidy up" 2>&1 | tail -20`, ok: true, kind: "codex", prompt: "tidy up"},
		{cmd: `codex --search exec "look it up"`, ok: true, kind: "codex", prompt: "look it up"},
		{cmd: `codex -m gpt-5 exec "tidy up"`, ok: true, kind: "codex", prompt: "tidy up", model: "gpt-5"},
		{cmd: `copilot -p "explain main.go" --allow-all-tools`, ok: true, kind: "copilot", prompt: "explain main.go"},
		{cmd: `CLAUDE_CONFIG_DIR=~/.claude-2 claude -p hi`, ok: true, kind: "claude", prompt: "hi"},
		{cmd: "nohup codex exec --sandbox read-only - \\\n  < \"$JOB/tmp/l1-prompt.md\" > \"$JOB/l1.log\" 2>&1 &", ok: true, kind: "codex", from: "$JOB/tmp/l1-prompt.md"},
		{cmd: `nohup claude -p "$(cat "$JOB/tmp/port-prompt.md")" --model claude-opus-4-6 > port.log 2>&1 &`, ok: true, kind: "claude", from: "$JOB/tmp/port-prompt.md", model: "claude-opus-4-6"},
		{cmd: `nohup codex exec -m $M -c model_reasoning_effort="$E" "$(cat $T/lanes/$L.txt)" > $T/$L.log 2>&1 < /dev/null &`, ok: true, kind: "codex", from: "$T/lanes/$L.txt", model: "$M"},
		{cmd: `claude -p --model haiku --input-format stream-json --permission-prompts host < in.jsonl > out.jsonl`, ok: true, kind: "claude", from: "in.jsonl", model: "haiku"},
		{cmd: "claude -p <<'EOF'\nwrite the plan\nEOF", ok: true, kind: "claude", prompt: "write the plan"},
		{cmd: "cat > run.sh <<'EOF'\nclaude -p \"hi\"\nEOF\nchmod +x run.sh"},
		{cmd: `ps -ax | grep 'claude -p'`},
		{cmd: `claude --version`},
		{cmd: codexCheck},
		{cmd: `claude -p --help`},
		{cmd: `claude mcp list`},
		{cmd: `codex login`},
		{cmd: `codex "interactive"`},
		{cmd: `claude`},
		{cmd: `which claude`},
		{cmd: `go test ./...`},
		{cmd: `rush session start --agent codex --name walk --cwd /work --prompt-file .claude/tmp/walk.md --json`, ok: true, kind: "codex", from: ".claude/tmp/walk.md", dir: "/work"},
		{cmd: "rush session start --agent codex --prompt-file - <<'EOF'\nwalk the site\nEOF", ok: true, kind: "codex", prompt: "walk the site"},
		{cmd: `rush session start --session-id x --resume`},
	} {
		sp, ok := SpawnOf(c.cmd)
		if ok != c.ok {
			t.Errorf("%q: spawn %v, want %v", c.cmd, ok, c.ok)
			continue
		}
		if !ok {
			continue
		}
		if string(sp.Kind) != c.kind || sp.Prompt != c.prompt || sp.From != c.from || sp.Dir != c.dir || sp.Model != c.model {
			t.Errorf("%q: got %+v", c.cmd, sp)
		}
	}
}

// A command that ran another agent draws as that agent, with the steps its
// own session took under it while it runs: the latest few until opened.
func TestSpawnRow(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work/rush"
	s.Apply(host.Sent{Text: "get a second opinion"}, at(0))
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": `codex exec -m gpt-5 "review the diff for races"`}), at(1))
	child := New()
	child.Model = "gpt-5"
	child.Apply(host.Sent{Text: "review the diff for races"}, at(1))
	for i, f := range []string{"a.go", "b.go", "c.go", "d.go", "e.go", "f.go"} {
		id := string(rune('a' + i))
		child.Apply(toolUse(id, "Read", map[string]any{"file_path": "/work/rush/" + f}), at(2+i))
		child.Apply(toolResult(id, "…", false, nil), at(2+i))
	}
	st := s.Spawns()
	if len(st) != 1 {
		t.Fatalf("spawns: %d", len(st))
	}
	s.SetChildren(st[0], []Child{{ID: "c", Sess: child}})
	out := plain(s.Render(Options{Width: 100, Now: at(9)}))
	for _, want := range []string{"Codex  review the diff for races", "6 steps", "⋯ 2 steps before", "c.go", "f.go"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	if strings.Contains(out, "b.go") {
		t.Errorf("an early step shows before the row is opened:\n%s", out)
	}
}

// Opened, a spawned agent's row shows what it said back, as any
// subagent's does: not the command that ran it, nor its banner.
func TestSpawnReplies(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "get a second opinion"}, at(0))
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": `codex exec "review the diff for races"`}), at(1))
	s.Apply(toolResult("b1", "OpenAI Codex v0.1\nworkdir: /work\nNo races found.", false, nil), at(5))
	child := New()
	child.Apply(host.Sent{Text: "review the diff for races"}, at(1))
	child.Apply(say("No races found."), at(4))
	s.SetChildren(s.Spawns()[0], []Child{{ID: "c", Sess: child}})
	out := plain(s.Render(Options{Width: 100, Now: at(9), Verbose: true}))
	if !strings.Contains(out, "No races found.") || strings.Contains(out, "$ codex exec") || strings.Contains(out, "workdir") {
		t.Errorf("the reply isn't its body:\n%s", out)
	}
}

// A harness's own subagent kept in a session apart (Codex's spawn_agent)
// is followed as a spawned agent is: its row is a subagent's.
func TestHarnessChildSpawn(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "fan out"}, at(0))
	s.Apply(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.ToolCall, Call: &tool.Call{ID: "c1", Name: "spawn_agent",
		Kind: tool.Subagent, Input: tool.Input{Description: "trace_bug", Child: "trace_bug"}}}}}, at(1))
	s.Apply(toolUse("c2", "Agent", map[string]any{"description": "inline", "prompt": "look"}), at(2))
	st := s.Spawns()
	if len(st) != 1 || st[0].ID != "c1" {
		t.Fatalf("spawns: %v", st)
	}
	if sp, _ := st[0].Spawn(); sp.Child != "trace_bug" || sp.Prompt != "trace_bug" {
		t.Errorf("spawn %+v", sp)
	}
}

// codexCheck only asks codex about itself: a help or version flag anywhere
// in an agent's arguments is no run of it.
const codexCheck = "command -v codex; codex --version; codex exec --help | head -40; grep model ~/.codex/config.toml"

// Nor, with no agent found, does it draw as one: it's the shell step it is.
func TestCheckIsAShellStep(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "is codex set up?"}, at(0))
	s.Apply(toolUse("b1", "Bash", map[string]any{"command": codexCheck, "description": "Check codex CLI and the astra model"}), at(1))
	s.Apply(toolResult("b1", "/usr/local/bin/codex\ncodex-cli 0.1", false, nil), at(2))
	out := plain(s.Render(Options{Width: 100, Now: at(3)}))
	if strings.Contains(out, "⇉") || !strings.Contains(out, "$ Check codex CLI and the astra model") {
		t.Errorf("drawn as an agent:\n%s", out)
	}
}

// A spawn_agent call is drawn as the agent it starts, its prompt as the
// brief and the answer as its reply; found, the agent's own steps go under it.
func TestSpawnToolCall(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "ask astra"}, at(0))
	s.Apply(toolUse("t1", "mcp__rush__spawn_agent", map[string]any{"agent": "astra", "prompt": "Review the diff for races"}), at(1))
	if d, _ := s.Doing(); d != "asking astra" {
		t.Errorf("doing: %q", d)
	}
	st := s.Step("t1")
	if sp, ok := st.Spawn(); !ok || sp.Name != "astra" || sp.Prompt != "Review the diff for races" {
		t.Errorf("spawn: %+v %v", sp, ok)
	}
	if ws := s.Windows(at(2), 0); len(ws) != 1 || ws[0].Asked != "Review the diff for races" {
		t.Errorf("windows: %+v", ws)
	}
	kid := New()
	kid.Apply(host.Sent{Text: "Review the diff for races"}, at(1))
	kid.Apply(toolUse("k1", "Bash", map[string]any{"command": "git diff", "description": "Read the diff"}), at(2))
	kid.Apply(toolResult("k1", "+x", false, nil), at(3))
	s.SetChildren(st, []Child{{ID: "c1", Spawn: Spawn{Kind: "codex", Name: "Codex", Prompt: "Review the diff for races"}, Sess: kid, Live: true, Start: at(1)}})
	s.Apply(toolResult("t1", "Codex (c1) finished:\n\nNo races found.", false, nil), at(5))
	out := plain(s.Render(Options{Width: 100, Now: at(9), Verbose: true}))
	for _, want := range []string{"Codex", "Review the diff for races", "Read the diff"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	t.Log("\n" + out)
	if strings.Contains(out, "mcp__rush") || strings.Contains(out, "spawn agent") {
		t.Errorf("drawn as the tool, not the agent:\n%s", out)
	}
}

// Open, a spawn_agent call's row names the agent and its task shows once
// under it, wrapped to the pane rather than cut at its edge.
func TestSpawnToolBriefWraps(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "ask astra"}, at(0))
	prompt := "Code review, read-only: do NOT edit files (other agents work in this repo; the working tree is shared) TAILWORD\n\nRepo: here."
	s.Apply(toolUse("t1", "mcp__rush__spawn_agent", map[string]any{"agent": "astra", "prompt": prompt}), at(1))
	out := plain(s.Render(Options{Width: 70, Now: at(2)}))
	t.Log("\n" + out)
	if n := strings.Count(out, "Code review, read-only"); n != 1 {
		t.Errorf("task drawn %d times", n)
	}
	if !strings.Contains(out, "TAILWORD") {
		t.Errorf("long line cut, not wrapped")
	}
}
