package convo

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/charmbracelet/x/ansi"
)

func TestPartOf(t *testing.T) {
	segs := segments("cd /x && FOO=1 go build ./... && go test ./internal/ui 2>&1 | tail -5 && ./deploy.sh prod && git status")
	for _, c := range []struct {
		args []string
		want int
	}{
		{[]string{"/usr/local/go/bin/go", "build", "./..."}, 1},
		{[]string{"go", "test", "./internal/ui"}, 2},
		{[]string{"tail", "-5"}, 2},
		{[]string{"rtk", "go", "test", "./internal/ui"}, 2},
		{[]string{"/bin/bash", "./deploy.sh", "prod"}, 3},
		{[]string{"git", "status"}, 4},
		{[]string{"/tmp/go-build123/ui.test"}, -1},
	} {
		if got := partOf(segs, c.args, 0); got != c.want {
			t.Errorf("partOf(%q) = %d, want %d", c.args, got, c.want)
		}
	}
	// The same command twice: the one the chain has reached.
	twice := segments("make && sleep 5 && make")
	if got := partOf(twice, []string{"make"}, 1); got != 2 {
		t.Errorf("a repeated command should be the one from the chain's place on, got %d", got)
	}
}

func runningChain(cmd string, start time.Time) (*Session, *Step) {
	s := New()
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	st := &Step{ID: "b1", Tool: "Bash", Input: in, Status: Running, Start: start}
	s.Turns = append(s.Turns, &Turn{Live: true, Items: []*Item{{Kind: KStep, Step: st}}})
	return s, st
}

func TestWatchShellsTimesEachPart(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	cmd := "go build ./... && go test ./... && git status"
	s, st := runningChain(cmd, t0)
	shell := func(kids ...ShellProc) []Shell {
		return []Shell{{Cmd: "/bin/zsh -c source x && eval '" + cmd + "' < /dev/null", Start: t0, Kids: kids}}
	}
	// make's own git is make's, not the chain's.
	s.WatchShells(shell(ShellProc{Args: []string{"go", "build", "./..."}, Start: t0, Kids: []ShellProc{{Args: []string{"git", "status"}, Start: t0}}}), t0.Add(time.Second))
	s.WatchShells(shell(ShellProc{Args: []string{"go", "build", "./..."}, Start: t0}), t0.Add(2*time.Second))
	s.WatchShells(shell(ShellProc{Args: []string{"go", "test", "./..."}, Start: t0.Add(2500 * time.Millisecond)}), t0.Add(3*time.Second))
	if r := st.parts[0]; r == nil || r.end != t0.Add(2500*time.Millisecond) {
		t.Fatalf("the build should end when the test starts: %+v", r)
	}
	if st.parts[2] != nil {
		t.Fatalf("git under go build isn't the chain's git status")
	}
	if got := st.runningPart(); got != "2/3 go test" {
		t.Errorf("runningPart = %q", got)
	}

	d := &drawer{s: s, t: s.Turns[0], o: Options{Width: 120, Now: t0.Add(10 * time.Second), Open: map[string]bool{}}, cw: 120}
	d.shellBody(st, cmd, 4)
	var rows []string
	for _, l := range d.lines {
		rows = append(rows, ansi.Strip(l.Text))
	}
	out := strings.Join(rows, "\n")
	if len(rows) != 3 || !strings.Contains(rows[0], "✓ 2.5s") || !strings.Contains(rows[1], "7s") || strings.Contains(rows[2], "✓") {
		t.Errorf("each part should say how long it ran, the running one still counting:\n%s", out)
	}
}

// A hook that rewrote the command still finds its shell, by when it started.
func TestWatchShellsRewrittenCommand(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	s, st := runningChain("git status && go vet ./...", t0)
	s.WatchShells([]Shell{{Cmd: "/bin/zsh -c eval 'rtk git status && rtk go vet ./...'", Start: t0.Add(200 * time.Millisecond),
		Kids: []ShellProc{{Args: []string{"rtk", "go", "vet", "./..."}, Start: t0.Add(time.Second)}}}}, t0.Add(2*time.Second))
	if st.parts[1] == nil {
		t.Fatalf("go vet should be seen running: %+v", st.parts)
	}
}

// A call a permission prompt held back still finds its shell.
func TestWatchShellsHeldBack(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	s, st := runningChain("git status && go vet ./...", t0)
	s.WatchShells([]Shell{{Cmd: "/bin/zsh -c eval 'rtk git status && rtk go vet ./...'", Start: t0.Add(40 * time.Second),
		Kids: []ShellProc{{Args: []string{"rtk", "go", "vet", "./..."}, Start: t0.Add(41 * time.Second)}}}}, t0.Add(42*time.Second))
	if st.parts[1] == nil {
		t.Fatalf("go vet should be seen running: %+v", st.parts)
	}
}

func TestRunningPart(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	for _, c := range []struct {
		cmd, then string
	}{
		{"go build ./... && go test ./...", "stops"},
		{"go build ./...; go test ./...", "carries on"},
		{"go build ./... || echo failed", "carries on"},
		{"cd x && go build ./...", "ends"},
	} {
		s, st := runningChain(c.cmd, t0)
		s.byID = map[string]*Step{st.ID: st}
		s.WatchShells([]Shell{{Cmd: "eval '" + c.cmd + "'", Start: t0,
			Kids: []ShellProc{{PID: 42, Args: []string{"go", "build", "./..."}, Start: t0}}}}, t0.Add(time.Second))
		rp, ok := s.RunningPart(st.ID)
		if !ok || rp.Command != "go build ./..." || rp.Then != c.then || len(rp.Procs) != 1 || rp.Procs[0].PID != 42 {
			t.Errorf("%q: RunningPart = %+v, %v; want then %q", c.cmd, rp, ok, c.then)
		}
	}
}

// A call sent to the background has returned, but its chain runs on: it
// still says which of its commands runs now.
func TestWatchShellsBackgroundChain(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	cmd := "go build ./... && go test ./..."
	s, st := runningChain(cmd, t0)
	st.Status, s.Turns[0].Live = OK, false
	s.byID = map[string]*Step{"b1": st}
	s.jobs = []*Job{{ID: "j1", ToolUseID: "b1", Type: "local_bash", Background: true}}
	s.WatchShells([]Shell{{Cmd: "/bin/zsh -c eval '" + cmd + "'", Start: t0, Kids: []ShellProc{{PID: 9, Args: []string{"go", "test", "./..."}, Start: t0}}}}, t0.Add(time.Second))
	if rp, ok := s.RunningPart("b1"); !ok || rp.At != 2 || rp.Of != 2 || rp.Command != "go test ./..." {
		t.Fatalf("RunningPart = %+v, %v", rp, ok)
	}
	s.jobs[0].Status = "completed"
	if _, ok := s.RunningPart("b1"); ok {
		t.Fatal("a finished task still runs a part")
	}
}

// Commands no look caught running still say how long they took: the gap
// they ran in, alone, or at most it when they shared it.
func TestPartMarksFillGaps(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	s, st := runningChain("a && b && c && d", t0)
	st.Status, st.End = OK, t0.Add(10*time.Second)
	st.parts = map[int]*partRun{2: {start: t0.Add(2 * time.Second), end: t0.Add(7 * time.Second)}}
	d := &drawer{s: s, o: Options{Now: t0.Add(20 * time.Second)}}
	marks := d.partMarks(st, 4)
	got := make([]string, 0, len(marks))
	for _, m := range marks {
		got = append(got, ansi.Strip(m))
	}
	if want := "✓ <2.0s|✓ <2.0s|✓ 5s|✓ 3.0s"; strings.Join(got, "|") != want {
		t.Fatalf("marks = %q, want %q", strings.Join(got, "|"), want)
	}
}

// A loop's command seen start again counts a run and times this one, not
// all of them since the first.
func TestWatchShellsLoop(t *testing.T) {
	t0 := time.Date(2026, 9, 28, 12, 0, 0, 0, time.UTC)
	cmd := "for i in $(seq 1 30); do curl -s x && break; sleep 10; done; echo ok"
	s, st := runningChain(cmd, t0)
	look := func(at time.Duration, p ShellProc) {
		s.WatchShells([]Shell{{Cmd: "eval '" + cmd + "'", Start: t0, Kids: []ShellProc{p}}}, t0.Add(at))
	}
	sleep := func(from time.Duration) ShellProc {
		return ShellProc{Args: []string{"sleep", "10"}, Start: t0.Add(from)}
	}
	look(2*time.Second, sleep(time.Second))
	look(9*time.Second, sleep(time.Second))
	look(13*time.Second, sleep(12*time.Second))
	k := -1
	for i, r := range st.parts {
		if r.runs > 0 {
			k = i
		}
	}
	if k < 0 || st.parts[k].runs != 2 || !st.parts[k].start.Equal(t0.Add(12*time.Second)) {
		t.Fatalf("sleep should be on its second run, from 12s: %+v", st.parts[k])
	}
	if got := st.runningPart(); !strings.HasSuffix(got, "sleep 10 · run 2") {
		t.Errorf("runningPart = %q", got)
	}
}

// A shell task whose call isn't in the conversation (a subagent's) is
// drawn from its name when that's its command, and not when it's words.
func TestJobCallStandsIn(t *testing.T) {
	s := New()
	now := time.Now()
	s.backgroundNow([]event.BackgroundTask{
		{ID: "b1", Type: "local_bash", Label: "cd /x; until grep -q exit= run.out; do sleep 10; done; cat run.out > /tmp/r.txt"},
		{ID: "b2", Type: "local_bash", Label: "visualdiff check: live flow failures"},
	}, now)
	s.jobCalls(now)
	b1, b2 := s.Job("b1"), s.Job("b2")
	if s.JobCommand(b1) == "" || s.byID[b1.ToolUseID] == nil {
		t.Errorf("a command's task should have a call standing in: %+v", b1)
	}
	if w := s.JobWrites(b1); len(w) != 1 || w[0] != "/tmp/r.txt" {
		t.Errorf("its files = %v", w)
	}
	if s.JobCommand(b2) != "" {
		t.Errorf("a description isn't a command: %q", s.JobCommand(b2))
	}
}

func TestWatchShellsNeutralExactCommand(t *testing.T) {
	now := time.Now()
	command := "go build ./... && go test ./..."
	for _, name := range []string{"exec_command", "run_shell_command", "Shell", "bash"} {
		t.Run(name, func(t *testing.T) {
			s, st := runningChain(command, now)
			st.Tool = name
			st.setCall(&tool.Call{ID: st.ID, Name: name, Kind: tool.Shell, Input: tool.Input{Command: command}})
			kids := []ShellProc{{PID: 42, Args: []string{"go", "test", "./..."}, Start: now}}
			s.WatchShells([]Shell{{Command: "unrelated MCP server", Start: now, Kids: kids}}, now)
			if len(st.parts) != 0 {
				t.Fatal("unrelated shell acquired call by timestamp")
			}
			s.WatchShells([]Shell{{Command: command, Start: now, Kids: kids}}, now.Add(time.Second))
			if st.parts[1] == nil || st.parts[1].procs[0].PID != 42 {
				t.Fatalf("neutral shell not tracked: %+v", st.parts)
			}
		})
	}
}

func TestJobProgressAgeIgnoresDuplicateHeartbeatAndResets(t *testing.T) {
	s := New()
	now := time.Now()
	s.applyJob(event.TaskStarted{ID: "job"}, now)
	progress := event.TaskProgress{ID: "job", Summary: "Compiling", Tokens: 12}
	s.applyJob(progress, now.Add(time.Second))
	s.applyJob(progress, now.Add(time.Minute))
	j := s.Job("job")
	if !j.ProgressAt.Equal(now.Add(time.Second)) {
		t.Fatal("duplicate heartbeat counted as progress")
	}
	progress.Tokens++
	s.applyJob(progress, now.Add(2*time.Minute))
	if !j.ProgressAt.Equal(now.Add(2 * time.Minute)) {
		t.Fatal("new work did not count as progress")
	}
	s.applyJob(event.TaskDone{ID: "job", Status: "completed"}, now.Add(3*time.Minute))
	s.applyJob(event.TaskStarted{ID: "job"}, now.Add(4*time.Minute))
	if !j.ProgressAt.IsZero() || j.Summary != "" || j.Tokens != 0 {
		t.Fatalf("restarted task retains stale progress: %+v", j)
	}
}
