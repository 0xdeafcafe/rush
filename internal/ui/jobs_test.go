package ui

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// What runs in the background shows in the dock with its latest output; a
// command the turn waits on shows in the conversation instead. ↑ picks one;
// x stops it, and ctrl+b backgrounds what the turn waits on.
func TestJobDock(t *testing.T) {
	out := filepath.Join(t.TempDir(), "b2.output")
	os.WriteFile(out, []byte("first line\nnpm warn\nbuilding\r50%\r100%\nlistening on :3000\n\n"), 0o644)
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b1", ToolUseID: "t1", Type: "local_bash", Description: "go test ./..."}, now.Add(-10*time.Second))
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now.Add(-time.Minute))
	s.Apply(headless.TaskStarted{ID: "b3", ToolUseID: "t3", Type: "local_bash", Description: "ls"}, now)
	s.Job("b2").OutputFile = out

	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}
	jobs := c.dockJobs()
	if len(jobs) != 1 || jobs[0].ID != "b2" {
		t.Fatalf("only what runs in the background: %d", len(jobs))
	}
	got := ansi.Strip(strings.Join(m.jobsPreview(c, jobs, 110), "\n"))
	for _, want := range []string{"npm run dev", "in background", "› listening on :3000"} {
		if !strings.Contains(got, want) {
			t.Fatalf("no %q in\n%s", want, got)
		}
	}
	if strings.Contains(got, "go test") || strings.Contains(got, "› ls") {
		t.Fatalf("what the turn waits on shows:\n%s", got)
	}

	// ↑ from the box: the nearest first.
	m.moveSel(c, -1)
	if c.sel != "job:b2" {
		t.Fatalf("↑ picked %q", c.sel)
	}
	if m.paneKey(tea.KeyPressMsg{Text: "b"}, "b"); !strings.Contains(m.status, "already in the background") {
		t.Fatalf("b on a background one: %q", m.status)
	}
	if cmd := m.paneKey(tea.KeyPressMsg{Text: "x"}, "x"); cmd == nil || !strings.Contains(m.status, "stopping the shell") {
		t.Fatalf("x: cmd %v, status %q", cmd != nil, m.status)
	}
	c.sel = ""
	if cmd := m.paneKey(tea.KeyPressMsg{}, "ctrl+b"); cmd == nil || !strings.Contains(m.status, "to the background") {
		t.Fatalf("ctrl+b: cmd %v, status %q", cmd != nil, m.status)
	}

	// Once it's in the background, ctrl+b is /btw's again.
	s.Apply(headless.TaskUpdated{ID: "b1", Backgrounded: new(true)}, now)
	s.Apply(headless.TaskUpdated{ID: "b3", Status: "completed"}, now)
	if _, used := m.jobKey(c, "ctrl+b", true); used {
		t.Fatal("ctrl+b taken with nothing to background")
	}

	// The background view has them all; enter opens one's output.
	s.Apply(headless.TaskDone{ID: "b1", Status: "completed"}, now)
	c.sel = "job:b2"
	view := ansi.Strip(joinLines(m.jobLines(c, convo.Options{Width: 110, Selected: c.sel})))
	for _, want := range []string{"Running", "npm run dev", "Finished", "go test ./...", "✓ done"} {
		if !strings.Contains(view, want) {
			t.Fatalf("no %q in\n%s", want, view)
		}
	}
	if !strings.Contains(view, "│ 100%") || strings.Contains(view, "│ first line") {
		t.Fatalf("closed, a running one shows other than its last lines:\n%s", view)
	}
	c.open["job:b2"] = true
	if view := ansi.Strip(joinLines(m.jobLines(c, convo.Options{Width: 110}))); !strings.Contains(view, "│ first line") {
		t.Fatalf("opened, no output:\n%s", view)
	}
}

func joinLines(ls []convo.Line) string {
	var b strings.Builder
	for _, l := range ls {
		b.WriteString(l.Text + "\n")
	}
	return b.String()
}

// A file read by a task that had ended is still read again for one that
// runs: tasks can share a file, and a running one's output keeps coming.
func TestTailSharedFileKeepsComing(t *testing.T) {
	p := filepath.Join(t.TempDir(), "run.err")
	os.WriteFile(p, []byte("one\n"), 0o644)
	c := &hostConn{}
	if got := c.tailOf(p, false, 5); len(got) != 1 || !c.tails[p].final {
		t.Fatalf("a finished task's read: %v, final %v", got, c.tails[p].final)
	}
	os.WriteFile(p, []byte("one\ntwo\n"), 0o644)
	os.Chtimes(p, time.Now().Add(time.Second), time.Now().Add(time.Second))
	c.tails[p].at = time.Time{} // due a look
	if got := c.tailOf(p, true, 5); len(got) != 2 {
		t.Fatalf("the running task's output stopped coming: %v", got)
	}
	if c.tailWhen(p, time.Now()) == "" {
		t.Error("no time for when it last changed")
	}
}

// In the background view the pointer lights a task's rows, and one click
// opens its output, another closes it.
func TestBackgroundHoverClick(t *testing.T) {
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c, paneFocus: true}
	for i, v := range m.views(c) {
		if v == "background" {
			c.view = i
		}
	}
	c.rowRefs = []string{"", "", "", "job:b2", "job:b2"}
	if _, cmd := m.subMouseMove(50, 3); cmd != nil || c.subHover != "job:b2" {
		t.Fatalf("hover: %q", c.subHover)
	}
	lines := m.jobLines(c, convo.Options{Width: 80, Open: c.open})
	lit := false
	for _, l := range lines {
		lit = lit || l.Ref == "job:b2" && strings.Contains(l.Text, hoverBG)
	}
	if !lit {
		t.Fatal("the task under the pointer isn't lit")
	}
	m.clickRow(c, 0, 4)
	if c.sel != "job:b2" || !c.open["job:b2"] {
		t.Fatalf("one click opens it: sel=%q open=%v", c.sel, c.open)
	}
	m.clickRow(c, 0, 3)
	if c.open["job:b2"] {
		t.Fatal("a second click closes it")
	}
}

// A running shell shows what it uses, in the dock and the background
// view: the process Claude started with it and those under it.
func TestJobUsage(t *testing.T) {
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 10, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now)
	tab := &proc.Table{At: now, Procs: map[int]*proc.Proc{
		10: {PID: 10, Comm: "claude", Start: now.Add(-time.Hour)},
		11: {PID: 11, PPID: 10, Comm: "node", Start: now.Add(-time.Hour), CPU: 90, Footprint: 900 << 20}, // an MCP server
		12: {PID: 12, PPID: 10, Comm: "zsh", Start: now.Add(time.Second), CPU: 1, Footprint: 2 << 20},
		13: {PID: 13, PPID: 12, Comm: "vite", Start: now.Add(2 * time.Second), CPU: 24, Footprint: 300 << 20},
	}, Children: map[int][]int{10: {11, 12}, 12: {13}}}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{"job:b2": true}}
	m := &Model{snap: &fleet.Snapshot{Table: tab}, host: c}
	dock := ansi.Strip(strings.Join(m.jobsPreview(c, s.RunningJobs(), 110), "\n"))
	if !strings.Contains(dock, "25% · 302M") {
		t.Fatalf("the dock row has no usage:\n%s", dock)
	}
	var page []string
	for _, l := range m.jobLines(c, convo.Options{Width: 110, Open: c.open}) {
		page = append(page, ansi.Strip(l.Text))
	}
	got := strings.Join(page, "\n")
	if !strings.Contains(got, "25% · 302M") || !strings.Contains(got, "vite") || strings.Contains(got, "node") {
		t.Fatalf("the view's usage and processes:\n%s", got)
	}
}

// A task still running whose output ended on a crash a while ago is
// called out as likely dead; one still writing, or that ended well, isn't.
func TestJobCrashed(t *testing.T) {
	out := filepath.Join(t.TempDir(), "b2.output")
	os.WriteFile(out, []byte("listening on :3000\nTypeError: x is undefined\n[nodemon] app crashed - waiting for file changes before starting...\n"), 0o644)
	old := time.Now().Add(-time.Minute)
	os.Chtimes(out, old, old)
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 1, State: "working"}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "npm run dev", Backgrounded: true}, now.Add(-2*time.Minute))
	s.Job("b2").OutputFile = out
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	if got := ansi.Strip(strings.Join(m.jobsPreview(c, s.RunningJobs(), 120), "\n")); !strings.Contains(got, "error in recent output") {
		t.Fatalf("no crash called out:\n%s", got)
	}
	os.Chtimes(out, now, now) // still writing: it may be restarting
	c.tails = nil
	if got := ansi.Strip(strings.Join(m.jobsPreview(c, s.RunningJobs(), 120), "\n")); strings.Contains(got, "error in recent output") {
		t.Fatalf("called crashed while still writing:\n%s", got)
	}
}

// An opened task has one bar down its left, beside its processes, its
// command and its output, so they read as inside it.
func TestOpenedJobEdge(t *testing.T) {
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, ClaudePID: 10, State: "working"}}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "t2", Name: "Bash",
		Input: []byte(`{"command":"for p in a b; do echo $p; done > out.log\ncat out.log","run_in_background":true}`)}}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "check the lot", Backgrounded: true}, now)
	tab := &proc.Table{At: now, Procs: map[int]*proc.Proc{
		10: {PID: 10, Comm: "claude", Start: now.Add(-time.Hour)},
		12: {PID: 12, PPID: 10, Comm: "zsh", Start: now.Add(time.Second), CPU: 1, Footprint: 2 << 20},
	}, Children: map[int][]int{10: {12}}}
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{"job:b2": true}}
	m := &Model{snap: &fleet.Snapshot{Table: tab}, host: c}
	var job []string
	for _, l := range m.jobLines(c, convo.Options{Width: 110, Open: c.open, Selected: "job:b2", Focused: true}) {
		if l.Ref == "job:b2" {
			job = append(job, ansi.Strip(l.Text))
		}
	}
	if len(job) < 4 {
		t.Fatalf("opened, too little:\n%s", strings.Join(job, "\n"))
	}
	if !strings.Contains(job[0], "▾ $ shell") {
		t.Errorf("opened, no ▾ before the task: %q", job[0])
	}
	delete(c.open, "job:b2")
	if l := ansi.Strip(m.jobLines(c, convo.Options{Width: 110})[3].Text); !strings.Contains(l, "▸ $ shell") {
		t.Errorf("closed, no ▸ before the task: %q", l)
	}
	for _, l := range job {
		if !strings.HasPrefix(l, "▍") {
			t.Errorf("a row of the opened task without the bar: %q\n%s", l, strings.Join(job, "\n"))
		}
	}
}

// A tail -f under a task says which files it follows, as the task's own
// output; any other command follows none.
func TestTailFollows(t *testing.T) {
	for cmd, want := range map[string]string{
		"tail -n0 -F .claude/tmp/runs/r6/fuzz.log": "/w/.claude/tmp/runs/r6/fuzz.log",
		"tail -f /var/log/a.log b.log":             "/var/log/a.log /w/b.log",
		"tail --follow=name x.log":                 "/w/x.log",
		"tail -20 .claude/tmp/runs/r6.out":         "",
		"grep -F x.log y":                          "",
	} {
		if got := strings.Join(tailFollows(cmd, "/w"), " "); got != want {
			t.Errorf("%q follows %q, want %q", cmd, got, want)
		}
	}
}

// An opened task names a file its command writes that's still empty, so
// no output reads as none yet rather than as nothing there.
func TestOpenedJobEmptyFile(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "r6.out"), nil, 0o644)
	s := convo.New()
	now := time.Now()
	s.Info.Cwd = dir
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, Cwd: dir, State: "working"}}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "t2", Name: "Bash",
		Input: []byte(`{"command":"bash run-all.sh > r6.out 2>&1","run_in_background":true}`)}}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "run it all", Backgrounded: true}, now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{"job:b2": true}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	m.jobLines(c, convo.Options{Width: 110, Open: c.open}) // the reads start
	if got := ansi.Strip(joinLines(m.jobLines(c, convo.Options{Width: 110, Open: c.open}))); !strings.Contains(got, "from r6.out · empty so far") {
		t.Fatalf("the empty file isn't named:\n%s", got)
	}
}

// A running task whose own output is empty doesn't show a file it wrote at
// its start and left as its output, which would read as done; a file it's
// still writing does stand for it.
func TestJobOutputOnlyLiveFile(t *testing.T) {
	dir := t.TempDir()
	spec := filepath.Join(dir, "spec.json")
	os.WriteFile(spec, []byte("{\n}\n"), 0o644)
	old := time.Now().Add(-2 * time.Minute)
	os.Chtimes(spec, old, old)
	s := convo.New()
	now := time.Now()
	s.Info.Cwd = dir
	s.Apply(host.InfoEvent{Info: host.Info{Proto: 3, Cwd: dir, State: "working"}}, now)
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "t2", Name: "Bash",
		Input: []byte(`{"command":"git show main:spec.json > spec.json; python3 check.py","run_in_background":true}`)}}}, now)
	s.Apply(headless.TaskStarted{ID: "b2", ToolUseID: "t2", Type: "local_bash", Description: "check it", Backgrounded: true}, now)
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	j := s.Job("b2")
	m.jobTailFrom(c, j, 3) // the reads start
	if lines, from := m.jobTailFrom(c, j, 3); from != "" {
		t.Fatalf("a file left since the start stands for the output: %s %q", from, lines)
	}
	os.WriteFile(spec, []byte("{\n\"more\": 1\n}\n"), 0o644)
	c.tails[spec].at = time.Time{} // look again now
	m.jobTailFrom(c, j, 3)
	if _, from := m.jobTailFrom(c, j, 3); from != spec {
		t.Fatalf("a file still being written doesn't stand for the output: %q", from)
	}
}

// An agent the shell ran in the background (codex exec, claude -p) is a
// subagent once rush has found its session: not a background command in
// the dock or the background view, and looked for while it still runs.
func TestSpawnedJobIsASubagent(t *testing.T) {
	s := convo.New()
	now := time.Now()
	s.Apply(headless.TaskStarted{ID: "b1", ToolUseID: "t1", Type: "local_bash", Description: "codex exec 'fix it'", Backgrounded: true}, now.Add(-time.Minute))
	c := &hostConn{kind: "claude", key: "k", sess: s, open: map[string]bool{}}
	j := s.Job("b1")
	if !s.JobRunning("t1") || c.jobKind(j) != "shell" || len(c.dockJobs()) != 1 {
		t.Fatalf("not found yet, it's the shell it is: %q", c.jobKind(j))
	}
	c.spawns = map[string]*spawnRun{"t1": {step: "t1", path: "/tmp/rollout.jsonl"}}
	if c.jobKind(j) != "subagent" || len(c.dockJobs()) != 0 {
		t.Fatalf("found, it's a subagent: %q, %d in the dock", c.jobKind(j), len(c.dockJobs()))
	}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	if slices.Contains(m.views(c), "background") {
		t.Error("no background view for it alone")
	}
}

// A running or opened shell step in the conversation shows what the files
// its command writes to hold, as the background view does.
func TestConversationShowsWrites(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "test.log"), []byte("ok  pkg/a\nFAIL pkg/b\n"), 0o644)
	s := convo.New()
	now := time.Now()
	s.Apply(host.InfoEvent{Info: host.Info{Cwd: dir}}, now)
	s.Apply(host.Sent{Text: "test it"}, now)
	s.Apply(event.Message{Role: "assistant", ID: "m1", Parts: []event.Part{
		{Kind: event.ToolCall, Call: &tool.Call{ID: "b1", Name: "Bash", Kind: tool.Shell, Input: tool.Input{Command: "go test ./... > test.log 2>&1"}}},
	}}, now)
	c := &hostConn{kind: "claude", key: "k", sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	body := s.Render(convo.Options{Width: 100, Now: now, Open: c.open})
	ref := ""
	for _, l := range body {
		if strings.Contains(l.Ref, ":s:b1") {
			ref = l.Ref
		}
	}
	c.open[ref] = false
	shut := s.Render(convo.Options{Width: 100, Now: now, Open: c.open})
	if len(m.withWrites(c, shut, 100)) <= len(shut) {
		t.Fatal("running, it shows what it writes without being opened")
	}
	c.open[ref] = true
	body = s.Render(convo.Options{Width: 100, Now: now, Open: c.open})
	out := m.withWrites(c, body, 100)
	var b strings.Builder
	for _, l := range out {
		b.WriteString(ansi.Strip(l.Text) + "\n")
	}
	text := b.String()
	if !strings.Contains(text, "from test.log") || !strings.Contains(text, "│ FAIL pkg/b") {
		t.Fatalf("no preview of test.log:\n%s", text)
	}
	if len(out) <= len(body) {
		t.Fatal("the preview's rows go in a copy")
	}
}

// shift+x on a task takes the box for why, esc gives the pick back, and
// enter stops it as x would.
func TestStopWithNote(t *testing.T) {
	s := convo.New()
	s.Apply(headless.TaskStarted{ID: "b1", ToolUseID: "t1", Type: "local_bash", Description: "sleep 100", Backgrounded: true}, time.Now())
	c := &hostConn{kind: "claude", key: "k", sess: s, open: map[string]bool{}, sel: "job:b1"}
	m := &Model{snap: &fleet.Snapshot{}, host: c}
	if _, used := m.stopNoteKey(c, "shift+x", true); !used || c.stopNote == nil || c.sel != "" {
		t.Fatalf("X should take the box: %+v %q", c.stopNote, c.sel)
	}
	m.stopNoteKey(c, "esc", true)
	if c.stopNote != nil || c.sel != "job:b1" {
		t.Fatalf("esc gives the pick back: %q", c.sel)
	}
	m.stopNoteKey(c, "X", true)
	m.stopNoteKey(c, "enter", true)
	if c.stopNote != nil || !strings.Contains(m.status, "stop a task only in a session it runs") {
		t.Errorf("enter stops it as x would: %q", m.status)
	}
	c.sel = ""
	if _, used := m.stopNoteKey(c, "X", true); used {
		t.Error("with nothing picked, X is only a letter")
	}
}
