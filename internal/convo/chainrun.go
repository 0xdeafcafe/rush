package convo

import (
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// Shell is a shell invocation observed under a harness process. Command is
// the exact -c argument when available; Cmd retains native wrapper metadata.
type Shell struct {
	Cmd     string
	Command string
	Start   time.Time
	Kids    []ShellProc
}

// ShellProc is a process under a Bash call's shell, and its own children.
type ShellProc struct {
	PID   int
	Args  []string
	Start time.Time
	Kids  []ShellProc
}

// partRun is when one command of a chain was seen running: from its
// process's start until it was gone, or the next command's start.
type partRun struct {
	start, end, seen time.Time
	procs            []PartProc // its processes when last seen
	// runs is how many times it was seen start: more than once in a loop.
	// ponytail: a quick command can start and end between looks and go
	// uncounted; a slow one (the loop's sleep) counts the rounds right.
	runs int
}

// PartProc is a process running a command of a chain, and its start, which
// tells it from a later process given the same pid.
type PartProc struct {
	PID   int
	Start time.Time
}

// WatchShells matches the shells Claude Code has running to the Bash calls
// still running, and each shell's processes to the commands of its chain,
// so the opened call can say which of them runs now and how long each ran.
func (s *Session) WatchShells(shells []Shell, now time.Time) {
	var steps []*Step
	if t := s.Live(); t != nil {
		var walk func(*Step)
		walk = func(st *Step) {
			if st.Status == Running && st.kind() == tool.Shell {
				steps = append(steps, st)
			}
			for _, c := range st.Children {
				walk(c)
			}
		}
		for _, it := range t.Items {
			if it.Kind == KStep {
				walk(it.Step)
			}
		}
	}
	// A call sent to the background has returned; its shell runs on.
	for _, j := range s.jobs {
		if st := s.byID[j.ToolUseID]; j.Running() && st != nil && st.Status != Running && st.kind() == tool.Shell {
			steps = append(steps, st)
		}
	}
	if len(steps) == 0 || len(shells) == 0 {
		return
	}
	// A shell's command line holds the call's command as eval quotes it;
	// one a hook rewrote goes to the call it started soonest after. A
	// permission prompt or classifier can hold a call back any time.
	used := make([]bool, len(shells))
	var left []*Step
	for _, st := range steps {
		q := "'" + strings.ReplaceAll(st.in().Command, "'", `'\''`) + "'"
		found := false
		for i, sh := range shells {
			if !used[i] && (sh.Command != "" && sh.Command == st.in().Command || sh.Command == "" && strings.Contains(sh.Cmd, q)) {
				used[i], found = true, true
				st.watch(sh, now)
				break
			}
		}
		if !found {
			left = append(left, st)
		}
	}
	for _, st := range left {
		best := -1
		for i, sh := range shells {
			if used[i] || st.Start.IsZero() || sh.Command != "" {
				continue
			}
			if sh.Start.After(st.Start.Add(-2*time.Second)) && (best < 0 || sh.Start.Before(shells[best].Start)) {
				best = i
			}
		}
		if best >= 0 {
			used[best] = true
			st.watch(shells[best], now)
		}
	}
}

// watch notes which commands of the step's chain sh has running now.
func (st *Step) watch(sh Shell, now time.Time) {
	segs := segments(st.in().Command)
	if len(segs) < 2 {
		return
	}
	// A process that is one of the chain's commands stands for all it
	// starts: make's git is make's, not the chain's git status.
	seen := map[int]time.Time{}
	procs := map[int][]PartProc{}
	var walk func([]ShellProc)
	walk = func(ps []ShellProc) {
		for _, p := range ps {
			if k := partOf(segs, p.Args, st.at); k >= 0 {
				if t, ok := seen[k]; !ok || p.Start.Before(t) {
					seen[k] = p.Start
				}
				procs[k] = append(procs[k], PartProc{PID: p.PID, Start: p.Start})
				continue
			}
			walk(p.Kids)
		}
	}
	walk(sh.Kids)
	if st.parts == nil {
		st.parts = map[int]*partRun{}
	}
	for k, t := range seen {
		r := st.parts[k]
		switch {
		case r == nil:
			r = &partRun{start: t, runs: 1}
			st.parts[k] = r
		case t.Before(r.start):
			r.start = t
		case t.After(r.start):
			// A new process for it: a loop has come round to it again.
			r.start = t
			r.runs++
		}
		r.seen, r.end, r.procs = now, time.Time{}, procs[k]
		st.at = max(st.at, k)
	}
	// A command no longer seen ended when the next one started, or else
	// between the last look and this one.
	for k, r := range st.parts {
		if _, ok := seen[k]; ok || !r.end.IsZero() {
			continue
		}
		r.end, r.procs = now, nil
		for j, t := range seen {
			if j > k && t.After(r.seen) && t.Before(r.end) {
				r.end = t
			}
		}
	}
}

// wrappers run the command that follows them, and how many words of their
// own come first.
var wrappers = map[string]int{"sudo": 0, "env": 0, "time": 0, "nice": 0, "nohup": 0, "command": 0, "exec": 0, "timeout": 1, "rtk": 0, "caffeinate": 0}

// bare is a command's words from its program on: past NAME=value
// assignments and the wrappers that run it, with the program's folder gone.
func bare(words []string) []string {
	for len(words) > 0 {
		w := words[0]
		switch n, ok := wrappers[filepath.Base(w)]; {
		case strings.Contains(w, "=") && !strings.HasPrefix(w, "="):
			words = words[1:]
		case ok:
			words = words[1:]
			for len(words) > 0 && strings.HasPrefix(words[0], "-") {
				words = words[1:]
			}
			if len(words) > n {
				words = words[n:]
			}
		default:
			out := append([]string{filepath.Base(w)}, words[1:]...)
			return out
		}
	}
	return nil
}

// partOf is which command of the chain a process runs: the one whose
// program it is and that shares most words with it, the earliest from
// part from on when several do equally; -1 for none.
func partOf(segs []segment, args []string, from int) int {
	argv := bare(args)
	if len(argv) == 0 {
		return -1
	}
	// A script runs as its interpreter: bash ./build.sh.
	progs := []string{argv[0]}
	if len(argv) > 1 && !strings.HasPrefix(argv[1], "-") {
		progs = append(progs, filepath.Base(argv[1]))
	}
	have := map[string]bool{}
	for _, a := range argv[1:] {
		have[a] = true
	}
	best, score := -1, 0
	for k, sg := range segs {
		stages := append([]string{sg.text}, sg.filters...)
		sc := 0
		for _, stage := range stages {
			w := bare(fieldsOf(stage))
			if len(w) == 0 || !contains(progs, w[0]) {
				continue
			}
			n := 1
			for _, a := range w[1:] {
				if have[unquote(a)] {
					n++
				}
			}
			sc = max(sc, n)
		}
		if sc > score || sc == score && sc > 0 && best < from && k >= from {
			best, score = k, sc
		}
	}
	return best
}

// partMarks are what the opened call says of each command of its chain:
// how long it ran, or runs; nil when none was seen to.
func (d *drawer) partMarks(st *Step, n int) []string {
	if len(st.parts) == 0 || n < 2 {
		return nil
	}
	live := d.s.live(st)
	end := st.End
	if live || end.IsZero() {
		end = d.o.Now
	}
	marks := make([]string, n)
	for k, r := range st.parts {
		if k >= n {
			continue
		}
		stop := r.end
		round := ""
		if r.runs > 1 {
			round = " · run " + strconv.Itoa(r.runs)
		}
		switch {
		case stop.IsZero() && live && d.o.Paused[st.ID]:
			marks[k] = paint(cYellow+bold, "⏸ paused")
			continue
		case stop.IsZero() && live:
			marks[k] = paint(cOrange, d.spin(d.o.Tick)+" "+d.since(r.start)+round)
			continue
		case stop.IsZero():
			stop = end
		}
		t := dur(max(0, stop.Sub(r.start))) + round
		if st.Status == Failed && k == st.at {
			marks[k] = paint(cRed, "✗ "+t)
		} else {
			marks[k] = paint(cGreen, "✓ "+t)
		}
	}
	// Commands that came and went between looks, before one that was seen,
	// ran; so did those after the last when the whole chain did. They ran
	// in the gap between the ones seen either side: all of it when alone
	// there, a share of it when not.
	last := st.at
	if st.Status == OK {
		last = n - 1
	}
	for k := 0; k <= last && k < n; k++ {
		if marks[k] != "" {
			continue
		}
		from, to, gap := st.Start, end, k
		for i := k - 1; i >= 0; i-- {
			if r := st.parts[i]; r != nil {
				from = r.end
				break
			}
		}
		for gap+1 < n && st.parts[gap+1] == nil && gap+1 <= last {
			gap++
		}
		if r := st.parts[gap+1]; r != nil {
			to = r.start
		}
		t := dur(max(0, to.Sub(from)))
		if gap > k {
			t = "<" + t
		}
		if from.IsZero() || to.Before(from) {
			t = ""
		}
		for i := k; i <= gap; i++ {
			marks[i] = paint(cGreen, strings.TrimSpace("✓ "+t))
		}
	}
	return marks
}

// runningPart is the command of a running chain that runs now, as its
// place in the chain and its first words: "2/4 go test"; "" when none is
// known.
func (st *Step) runningPart() string {
	var at []int
	for k, r := range st.parts {
		if r.end.IsZero() {
			at = append(at, k)
		}
	}
	if len(at) == 0 {
		return ""
	}
	sort.Ints(at)
	segs := segments(st.in().Command)
	k := at[len(at)-1]
	if k >= len(segs) {
		return ""
	}
	w := bare(fieldsOf(segs[k].text))
	out := strings.Join(append([]string{strconv.Itoa(k+1) + "/" + strconv.Itoa(len(segs))}, w[:min(2, len(w))]...), " ")
	if n := st.parts[k].runs; n > 1 {
		out += " · run " + strconv.Itoa(n)
	}
	return out
}

// RunningPart is the command of a Bash call's chain that runs now.
type RunningPart struct {
	Command string     // as written: go test ./internal/ui
	Procs   []PartProc // the processes running it
	// Then is what the chain does once it's gone: "stops" (the next
	// command runs only if it succeeded), "carries on", or "ends" (it was
	// the last).
	Then   string
	At, Of int // its place in the chain: 2 of 5
}

// live is whether a call's command still runs: the call itself, or the
// task it went on as in the background.
func (s *Session) live(st *Step) bool {
	if st.Status == Running || s == nil {
		return st.Status == Running
	}
	return s.JobRunning(st.ID)
}

// JobLines is a shell task's command as an opened call draws it: a
// command a line, each with how long it ran, the one running now
// brighter; nil when the call that started it isn't in the conversation.
func (s *Session) JobLines(j *Job, o Options, indent int) []Line {
	st, cmd := s.byID[j.ToolUseID], s.JobCommand(j)
	if st == nil || cmd == "" {
		return nil
	}
	d := drawer{s: s, t: &Turn{}, o: o, cw: min(o.Width, o.rowCap())}
	d.shellBody(st, cmd, indent)
	return d.lines
}

// RunningPart is what the Bash call with this tool call ID runs now, if
// it's a chain seen running.
func (s *Session) RunningPart(id string) (RunningPart, bool) {
	st := s.Step(id)
	if st == nil || !s.live(st) || st.toolRun() != nil {
		return RunningPart{}, false
	}
	k := -1
	for i, r := range st.parts {
		if r.end.IsZero() && len(r.procs) > 0 && i > k {
			k = i
		}
	}
	if k < 0 {
		return RunningPart{}, false
	}
	cmd := st.in().Command
	segs := segments(cmd)
	if k >= len(segs) {
		return RunningPart{}, false
	}
	rp := RunningPart{Command: segs[k].text, Procs: st.parts[k].procs, Then: "ends", At: k + 1, Of: len(segs)}
	// The next command's line starts with what joins it on.
	n := -1
	for _, l := range shellLines(cmd) {
		if l.verbatim || l.depth > 0 {
			continue
		}
		if n++; n == k+1 {
			rp.Then = "carries on"
			if strings.HasPrefix(l.text, "&& ") {
				rp.Then = "stops"
			}
			break
		}
	}
	return rp, true
}
