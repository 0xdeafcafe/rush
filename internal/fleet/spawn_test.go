package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	_ "github.com/0xdeafcafe/rush/internal/adapters/codex"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// A claude -p a session's shell ran is that session's: found through the
// shell between them, and counted with its subagents. One a session's
// process runs itself (a host's Claude Code) isn't.
func TestSpawnOf(t *testing.T) {
	tab := &proc.Table{Procs: map[int]*proc.Proc{
		10: {PID: 10, PPID: 1},  // rush host
		11: {PID: 11, PPID: 10}, // its Claude Code
		12: {PID: 12, PPID: 11}, // zsh -c
		13: {PID: 13, PPID: 12}, // claude -p
		20: {PID: 20, PPID: 1},  // claude -p from a terminal
	}}
	parents := map[int]bool{10: true, 11: true, 13: true, 20: true}
	if got := spawnOf(tab, 13, parents); got != 11 {
		t.Errorf("spawn's parent %d, want 11", got)
	}
	if got := spawnOf(tab, 11, parents); got != 0 {
		t.Errorf("a host's own Claude Code taken for a spawn of %d", got)
	}
	if got := spawnOf(tab, 20, parents); got != 0 {
		t.Errorf("a terminal's claude -p taken for a spawn of %d", got)
	}
	host := &Agent{Key: "h", PID: 10}
	l := &Loader{links: map[string]string{}}
	l.foldSpawns(tab, []*Agent{host}, []spawn{{"c", 13}}, parents)
	if host.Subs.Direct != 1 {
		t.Errorf("host's subagents %+v", host.Subs)
	}
}

// A codex exec a Claude Code session's shell ran is that session's while
// it runs, though Codex says no process: found by when its process
// started. Once finished it stays the session's.
func TestSpawnedCodex(t *testing.T) {
	at := time.Now().Add(-time.Minute)
	tab := &proc.Table{Procs: map[int]*proc.Proc{
		11: {PID: 11, PPID: 1, Comm: "claude", Start: at.Add(-time.Hour)},
		12: {PID: 12, PPID: 11, Comm: "zsh", Start: at},
		13: {PID: 13, PPID: 12, Comm: "codex", Start: at.Add(time.Second)},
		20: {PID: 20, PPID: 1, Comm: "codex", Start: at.Add(-time.Hour)}, // codex in a terminal
	}}
	parent := &Agent{Key: "default/i:parent00", PID: 11, Kind: "claude", Interactive: true}
	kid := &Agent{Key: "codex/i:kid00000", Kind: "codex", Interactive: true, Headless: true}
	term := &Agent{Key: "codex/i:term0000", Kind: "codex", Interactive: true}
	kid.CreatedAt, term.CreatedAt = at, at.Add(-time.Hour)
	l := &Loader{links: map[string]string{}}
	got := l.foldSpawns(tab, []*Agent{parent, kid, term}, nil, map[int]bool{11: true})
	if len(got) != 2 || got[0] != parent || got[1] != term {
		t.Fatalf("listed %v", keys(got))
	}
	if parent.Subs.Direct != 1 || parent.Subs.Spawned != 1 {
		t.Errorf("parent's subagents %+v", parent.Subs)
	}
	if l.links[kid.Key] != parent.Key || !l.linksDirty {
		t.Errorf("links %v", l.links)
	}

	// Finished: no process, a past row, folded by the link.
	parent = &Agent{Key: "default/i:parent00", Kind: "claude", Past: true}
	kid = &Agent{Key: "codex/i:kid00000", Kind: "codex", Past: true}
	got = l.foldSpawns(&proc.Table{Procs: map[int]*proc.Proc{}}, []*Agent{parent, kid}, nil, map[int]bool{})
	if len(got) != 1 || got[0] != parent || parent.Subs.Spawned != 1 || parent.Subs.Direct != 0 {
		t.Errorf("listed %v, parent's subagents %+v", keys(got), parent.Subs)
	}
	// One whose session isn't listed stays a row of its own.
	kid = &Agent{Key: "codex/i:kid00000", Kind: "codex", Past: true}
	if got = l.foldSpawns(nil, []*Agent{kid}, nil, map[int]bool{}); len(got) != 1 {
		t.Errorf("an orphan folded away: %v", keys(got))
	}
}

func keys(as []*Agent) []string {
	out := make([]string, 0, len(as))
	for _, a := range as {
		out = append(out, a.Key)
	}
	return out
}

// A run an agent's stand-in hosted is listed with the rush session that
// ran it, running or not; one another agent ran is found by its process.
func TestHostedSpawns(t *testing.T) {
	parent := &Agent{Key: "default/a:parent00", Rush: true}
	parent.ID = "parent00"
	kid := &Agent{Key: "codex/a:kid00000", Rush: true}
	kid.ID = "kid00000"
	orphan := &Agent{Key: "codex/a:orphan00", Rush: true, PID: 40}
	orphan.ID = "orphan00"
	hosted := []host.Info{
		{ID: "parent00"},
		{ID: "kid00000", Meta: map[string]string{"spawnedBy": "parent00"}},
		{ID: "orphan00", Meta: map[string]string{"spawnedBy": "shell"}},
	}
	l := &Loader{links: map[string]string{}}
	agents := []*Agent{parent, kid, orphan}
	spawned := l.hostedSpawns(hosted, agents, nil)
	if len(spawned) != 1 || spawned[0] != (spawn{orphan.Key, 40}) {
		t.Errorf("found by process %v", spawned)
	}
	got := l.foldSpawns(&proc.Table{Procs: map[int]*proc.Proc{}}, agents, spawned, map[int]bool{})
	if len(got) != 2 || got[0] != parent || got[1] != orphan || parent.Subs.Spawned != 1 {
		t.Errorf("listed %v, parent's subagents %+v", keys(got), parent.Subs)
	}
	if parent.Busy() {
		t.Error("a finished spawn keeps its parent busy")
	}

	// A spawn still working keeps its idle parent out of Your turn.
	parent.Subs.Spawned = 0
	parent.State, parent.UpdatedAt, kid.State = "done", time.Now(), "working"
	l.foldSpawns(&proc.Table{Procs: map[int]*proc.Proc{}}, []*Agent{parent, kid}, nil, map[int]bool{})
	if !parent.Busy() || parent.YourTurn(time.Now()) {
		t.Errorf("parent with a working spawn: busy %v, your turn %v", parent.Busy(), parent.YourTurn(time.Now()))
	}

	// A spawn asking you something stays listed, so its ask can be seen.
	kid.State = "blocked"
	if got := l.foldSpawns(&proc.Table{Procs: map[int]*proc.Proc{}}, []*Agent{parent, kid}, nil, map[int]bool{}); len(got) != 2 {
		t.Errorf("a blocked spawn folded away: %v", keys(got))
	}
}

// A finished codex exec never seen running is found by its prompt in the
// transcript of the session working when it began, written just before,
// and folded from then on; one no session asked is looked for once.
func TestSpawnAskedBy(t *testing.T) {
	dir := t.TempDir()
	at := time.Now().Add(-time.Hour).UTC()
	prompt := "Make version 2 of the macOS app icon for a developer tool"
	line := func(ts time.Time, text string) string {
		return `{"type":"assistant","timestamp":"` + ts.Format(time.RFC3339Nano) + `","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"codex exec \"` + text + `\""}}]}}` + "\n"
	}
	parent := filepath.Join(dir, "parent.jsonl")
	later := filepath.Join(dir, "later.jsonl") // says it after the run began
	os.WriteFile(parent, []byte(line(at.Add(-2*time.Second), prompt)), 0o600)
	os.WriteFile(later, []byte(line(at.Add(time.Minute), prompt)), 0o600)
	mk := func(key, path string) *Agent {
		a := &Agent{Key: key, Kind: "claude", Past: true}
		a.TranscriptPath, a.CreatedAt, a.UpdatedAt = path, at.Add(-time.Hour), at.Add(time.Hour)
		return a
	}
	kid := &Agent{Key: "codex/i:kid00000", Kind: "codex", Past: true, Headless: true}
	kid.Name, kid.CreatedAt, kid.UpdatedAt = prompt+" called RUSH…", at, at.Add(time.Minute)
	lone := &Agent{Key: "codex/i:lone0000", Kind: "codex", Past: true, Headless: true}
	lone.Name, lone.CreatedAt = "Something nobody here ever asked for", at
	l := &Loader{links: map[string]string{}}
	// Another run of the same prompt holds it too, and ran nothing.
	retry := mk("codex/i:retry000", parent)
	retry.Name, retry.Kind, retry.Headless = kid.Name[:30]+" something else", "codex", true
	rows := func() []*Agent {
		return []*Agent{retry, mk("default/i:later000", later), mk("default/i:parent00", parent), kid, lone}
	}
	l.foldSpawns(nil, rows(), nil, map[int]bool{})
	deadline := time.Now().Add(5 * time.Second)
	for {
		l.takeIn()
		if _, ok := l.links[lone.Key]; ok && l.links[kid.Key] != "" || time.Now().After(deadline) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if l.links[kid.Key] != "default/i:parent00" {
		t.Fatalf("links %v", l.links)
	}
	if p, ok := l.links[lone.Key]; !ok || p != "" {
		t.Errorf("the lone run's %q %v", p, ok)
	}
	got := l.foldSpawns(nil, rows(), nil, map[int]bool{})
	if len(got) != 4 || got[2].Subs.Spawned != 1 {
		t.Errorf("listed %v", keys(got))
	}
}

// A run in a worktree of its own says which; one in its session's
// checkout says nothing.
func TestSubWorktree(t *testing.T) {
	dir := t.TempDir()
	repo, wt, gd := filepath.Join(dir, "repo"), filepath.Join(dir, "repo", ".claude", "worktrees", "agent-1"), filepath.Join(dir, "gd")
	os.MkdirAll(filepath.Join(repo, ".git"), 0o755)
	os.WriteFile(filepath.Join(repo, ".git", "HEAD"), []byte("ref: refs/heads/main\n"), 0o600)
	os.MkdirAll(wt, 0o755)
	os.MkdirAll(gd, 0o755)
	os.WriteFile(filepath.Join(wt, ".git"), []byte("gitdir: "+gd+"\n"), 0o600)
	os.WriteFile(filepath.Join(gd, "HEAD"), []byte("ref: refs/heads/worktree-agent-1\n"), 0o600)
	run := func(name, cwd string) string {
		p := filepath.Join(dir, name)
		os.WriteFile(p, []byte(`{"type":"user","cwd":"`+cwd+`"}`+"\n"), 0o600)
		return p
	}
	if got, ok := SubWorktree(run("a.jsonl", wt), repo); !ok || got != "worktree-agent-1" {
		t.Errorf("worktree run: %q %v", got, ok)
	}
	if got, ok := SubWorktree(run("b.jsonl", filepath.Join(repo, "sub")), repo); !ok || got != "" {
		t.Errorf("same checkout: %q %v", got, ok)
	}
	if _, ok := SubWorktree(filepath.Join(dir, "none.jsonl"), repo); ok {
		t.Error("no transcript told")
	}
}
