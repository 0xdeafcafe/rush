package convo

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Job is one of Claude Code's tasks: a Bash command, a monitor, a
// subagent or a workflow, whether the turn waits on it or it runs in the
// background. Only a session rush runs hears of them.
type Job struct {
	ID         string
	ToolUseID  string // the tool call that started it
	Type       string // local_bash, local_agent, monitor_mcp, local_workflow, ...
	Label      string // what Claude Code calls it: a description, or the command
	Agent      string // a subagent's type
	Background bool   // runs beside the turn rather than holding it
	// Status is empty while it runs; then completed, failed, stopped, or
	// ended when it went without a word (its process did).
	Status     string
	Error      string
	Start, End time.Time
	OutputFile string // where its output is, once it's done
	Summary    string // its latest progress, or how it ended
	LastTool   string
	ProgressAt time.Time // last reported change in progress, not a heartbeat
	Tokens     int
	ToolUses   int
}

func (j *Job) Running() bool { return j.Status == "" }

// Kind is what it is, in a word: shell, monitor, subagent, workflow, task.
func (j *Job) Kind() string {
	switch j.Type {
	case "local_bash":
		return "shell"
	case "monitor_mcp", "monitor_ws":
		return "monitor"
	case "local_agent", "remote_agent", "in_process_teammate":
		return "subagent"
	case "local_workflow":
		return "workflow"
	}
	return "task"
}

// Jobs are every task heard of, in the order they started.
func (s *Session) Jobs() []*Job { return s.jobs }

// WorkJobs are the tasks but subagents' runs: the shells, monitors and
// workflows the background view lists, in the order they started.
func (s *Session) WorkJobs() []*Job {
	var out []*Job
	for _, j := range s.jobs {
		if s.JobKind(j) != "subagent" {
			out = append(out, j)
		}
	}
	return out
}

// SubagentJob is the task of the subagent run with this agent id (Claude
// Code's task id for it) or started by this tool call, or nil.
func (s *Session) SubagentJob(id, toolUseID string) *Job {
	for _, j := range s.jobs {
		if s.JobKind(j) == "subagent" && (j.ID == id || toolUseID != "" && j.ToolUseID == toolUseID) {
			return j
		}
	}
	return nil
}

// SubagentJobs are the subagent runs' tasks by agent id and by the tool
// call that started each: SubagentJob for many runs at once, as a view
// asks for every run's on every frame.
func (s *Session) SubagentJobs() map[string]*Job {
	if s == nil {
		return map[string]*Job{}
	}
	out := map[string]*Job{}
	for _, j := range s.jobs {
		if s.JobKind(j) != "subagent" {
			continue
		}
		if _, ok := out[j.ID]; !ok {
			out[j.ID] = j
		}
		if j.ToolUseID != "" {
			if _, ok := out["call:"+j.ToolUseID]; !ok {
				out["call:"+j.ToolUseID] = j
			}
		}
	}
	return out
}

// RunningJobs are the tasks still running, in the order they started.
func (s *Session) RunningJobs() []*Job {
	var out []*Job
	for _, j := range s.jobs {
		if j.Running() {
			out = append(out, j)
		}
	}
	return out
}

// JobRunning is whether the task tool call id started is still running:
// a command sent to the background, long after its step returned.
func (s *Session) JobRunning(id string) bool {
	for _, j := range s.jobs {
		if j.ToolUseID == id && j.Running() {
			return true
		}
	}
	return false
}

// Job is the task with this id, or nil.
func (s *Session) Job(id string) *Job {
	for _, j := range s.jobs {
		if j.ID == id {
			return j
		}
	}
	return nil
}

func (s *Session) job(id string, now time.Time) *Job {
	if j := s.Job(id); j != nil {
		return j
	}
	j := &Job{ID: id, Start: now}
	s.jobs = append(s.jobs, j)
	return j
}

// jobStatus is Claude Code's word for how a task ended, as rush says it.
func jobStatus(st string) string {
	switch st {
	case "killed", "stopped":
		return "stopped"
	case "", "running", "pending":
		return ""
	}
	return st
}

func (s *Session) applyJob(ev event.Event, now time.Time) {
	switch ev := ev.(type) {
	case event.TaskStarted:
		j := s.job(ev.ID, now)
		s.reopenJob(j, now)
		j.ToolUseID, j.Type, j.Background = ev.CallID, taskType(ev.Kind), ev.Background
		j.Label = firstNonEmpty(ev.Label, j.Label)
		j.Agent = firstNonEmpty(ev.Agent, j.Agent)
		s.jobCalls(now)
	case event.TaskUpdated:
		j := s.job(ev.ID, now)
		if ev.Background != nil {
			j.Background = *ev.Background
		}
		if ev.Label != "" {
			j.Label = ev.Label
		}
		if ev.Err != "" {
			j.Error = ev.Err
		}
		if st := jobStatus(ev.Status); st != "" && (j.Running() || j.Status == "ended") {
			j.Status, j.End = st, now
		}
	case event.TaskProgress:
		j := s.job(ev.ID, now)
		if ev.Summary != "" && ev.Summary != j.Summary || ev.LastTool != "" && ev.LastTool != j.LastTool || ev.Tokens > j.Tokens || ev.ToolUses > j.ToolUses {
			j.ProgressAt = now
		}
		j.Summary, j.LastTool = firstNonEmpty(ev.Summary, j.Summary), firstNonEmpty(ev.LastTool, j.LastTool)
		j.Tokens, j.ToolUses = max(j.Tokens, ev.Tokens), max(j.ToolUses, ev.ToolUses)
		if s.JobKind(j) == "monitor" {
			s.noteWake(j, now) // each thing a monitor sees can wake the agent
		}
	case event.TaskDone:
		j := s.job(ev.ID, now)
		j.ToolUseID = firstNonEmpty(j.ToolUseID, ev.CallID)
		j.OutputFile = firstNonEmpty(ev.OutputFile, j.OutputFile)
		if st := firstNonEmpty(jobStatus(ev.Status), "completed"); j.Running() || j.Status == "ended" {
			j.Status = st
		}
		if j.End.IsZero() {
			j.End = now
		}
		if ev.Summary != "" && ev.Summary != j.Label {
			j.Summary = ev.Summary
		}
		s.TaskStatus[ev.ID] = firstNonEmpty(j.Status, "completed")
		s.noteWake(j, now)
	case event.Background:
		defer s.jobCalls(now)
		list := make([]event.BackgroundTask, 0, len(ev.Tasks))
		for _, t := range ev.Tasks {
			if t.Kind != event.OtherTask {
				t.Type = taskType(t.Kind)
			}
			list = append(list, t)
		}
		s.backgroundNow(list, now)
	}
}

// reopenJob starts again a task that had ended: a subagent a message was
// sent to after it finished runs again under the same id, and Claude Code
// starts it (and lists it in the background) anew.
func (s *Session) reopenJob(j *Job, now time.Time) {
	if j.Running() {
		return
	}
	j.Status, j.Error, j.End, j.Start, j.OutputFile = "", "", time.Time{}, now, ""
	j.ProgressAt = time.Time{}
	j.Summary, j.LastTool, j.Tokens, j.ToolUses = "", "", 0, 0
	delete(s.TaskStatus, j.ID)
}

// backgroundNow takes Claude Code's list of what runs in the background:
// each one there is running and backgrounded, and a backgrounded one no
// longer there has ended (how comes after, if Claude Code says).
func (s *Session) backgroundNow(list []event.BackgroundTask, now time.Time) {
	on := map[string]bool{}
	for _, t := range list {
		on[t.ID] = true
		j := s.job(t.ID, now)
		s.reopenJob(j, now)
		j.Background = true
		j.Type = firstNonEmpty(j.Type, t.Type)
		j.Label = firstNonEmpty(j.Label, t.Label)
	}
	for _, j := range s.jobs {
		if j.Running() && j.Background && !on[j.ID] {
			j.Status, j.End = "ended", now
		}
	}
}

// syncJobs matches the tasks to what the host says runs in the background,
// which a replay that no longer reaches back to a task's start still has.
// A host older than Proto 3 says nothing, so nothing is taken from it.
func (s *Session) syncJobs(info host.Info, now time.Time) {
	if info.Proto < 3 {
		return
	}
	if info.ClaudePID == 0 {
		// Claude Code isn't running: nothing it started is.
		for _, j := range s.jobs {
			if j.Running() {
				j.Status, j.End = "ended", now
			}
		}
		return
	}
	list := make([]event.BackgroundTask, 0, len(info.Background))
	for _, t := range info.Background {
		list = append(list, event.BackgroundTask{ID: t.ID, Kind: event.OtherTask, Type: t.Type, Label: t.Label})
		if s.Job(t.ID) == nil {
			s.job(t.ID, t.StartedAt)
		}
	}
	s.backgroundNow(list, now)
	s.jobCalls(now)
}

// jobCalls stands a call in for each shell task whose own isn't in the
// conversation, a subagent's or one from before a replay reaches, when
// what it's called is its command (as it is when it was given no
// description): so it's drawn, timed and its files found as any other.
func (s *Session) jobCalls(now time.Time) {
	for _, j := range s.jobs {
		if j.Type != "local_bash" || s.byID[j.ToolUseID] != nil || !looksLikeCommand(j.Label) {
			continue
		}
		// Under its call's id when it has one, so the call itself, should
		// it come after all, takes its place.
		if j.ToolUseID == "" {
			j.ToolUseID = "job:" + j.ID
		}
		in, _ := jsonx.Marshal(map[string]string{"command": j.Label})
		st := &Step{ID: j.ToolUseID, Tool: "Bash", Kind: tool.Shell, Input: in, Status: OK, Start: firstTime(j.Start, now), Exit: -1}
		st.read()
		s.byID[st.ID] = st
	}
}

// looksLikeCommand is whether a task's name is a shell command and not a
// description of one: a description is words, a command has a shell's
// punctuation, a path or a flag in it.
// ponytail: a guess; a one-word command with no flags (make) reads as words.
func looksLikeCommand(s string) bool {
	for _, mark := range []string{"&&", "||", ";", "|", ">", "$(", "/", " -", "=", "\n"} {
		if strings.Contains(s, mark) {
			return true
		}
	}
	return false
}

func firstTime(ts ...time.Time) time.Time {
	for _, t := range ts {
		if !t.IsZero() {
			return t
		}
	}
	return time.Time{}
}

// wakeWindow is how soon after a task ends or fires a turn with no message
// is put down to it.
const wakeWindow = 2 * time.Minute

// noteWake remembers a task that ended or fired: Claude Code wakes the
// agent for it without a message, and the turn it starts says so. One that
// comes while a turn runs is held until the turn ends, and wakes it then.
func (s *Session) noteWake(j *Job, now time.Time) {
	s.woke, s.wokeAt = j, now
}

// wake says who started t, a turn its task woke: the task, or the agents a
// shell task ran, once they're found, as a subagent's report back.
func (s *Session) wake(t *Turn) {
	j := t.wokeBy
	t.From, t.Cause, t.Command, t.replied = s.wakeFrom(j), firstNonEmpty(j.Label, j.Summary, s.JobCommand(j), j.ID), "", ""
	if st := s.byID[j.ToolUseID]; st != nil && st.ranAgents() {
		t.Cause, t.replied = firstNonEmpty(oneLine(st.in().Description), st.agentsAsked()), st.agentNames()
	} else if k := s.JobKind(j); (k == "shell" || k == "monitor") && strings.Contains(strings.TrimSpace(t.Cause), "\n") {
		// A shell task's description is its command, heredoc and all.
		t.Command, t.Cause = strings.TrimSpace(t.Cause), firstLine(t.Cause)
	}
	t.touch()
}

// wakeFrom is who started a turn a task woke: "background shell ·
// completed", "monitor · fired".
func (s *Session) wakeFrom(j *Job) string {
	kind := s.JobKind(j)
	if st := s.byID[j.ToolUseID]; st != nil && st.ranAgents() {
		kind = "subagent"
		if st.fan {
			kind = "subagents"
		}
	}
	if kind == "monitor" && j.Running() {
		return "monitor · fired"
	}
	from := kind
	if kind == "shell" || strings.HasPrefix(kind, "subagent") || kind == "workflow" {
		from = "background " + kind
	}
	if j.Status != "" {
		from += " · " + j.Status
	}
	return from
}

// endJobs ends the tasks a finished turn was waiting on: they ran in the
// foreground, so they were done by the time it was.
func (s *Session) endJobs(now time.Time) {
	for _, j := range s.jobs {
		if j.Running() && !j.Background {
			j.Status, j.End = "completed", now
			if st := s.byID[j.ToolUseID]; st != nil && st.Status == Failed {
				j.Status = "failed"
			}
		}
	}
}

// JobCommand is what a shell or monitor task runs, from the tool call that
// started it; empty when that call isn't in the conversation.
func (s *Session) JobCommand(j *Job) string {
	st := s.byID[j.ToolUseID]
	if st == nil {
		return ""
	}
	return strings.TrimSpace(st.in().Command)
}

// JobKind is Kind, knowing a Monitor tool's command from a Bash one: both
// are shell tasks to Claude Code. A task heard of only from its end (a
// replay that starts after it did) is known by the tool call that started it.
func (s *Session) JobKind(j *Job) string {
	switch j.Type {
	case "local_bash":
		if st := s.byID[j.ToolUseID]; st != nil && st.Tool == "Monitor" {
			return "monitor"
		}
	case "":
		if st := s.byID[j.ToolUseID]; st != nil && (st.Tool == "Agent" || st.Tool == "Task") {
			return "subagent"
		}
	}
	return j.Kind()
}

var (
	writeRe = regexp.MustCompile(`(?:^|\s)(?:\d|&)?>>?\s*([^\s;&|<>()]+)`)
	teeRe   = regexp.MustCompile(`\btee\s+(?:-a\s+)?([^\s;&|<>()]+)`)
)

// JobWrites are the files a shell task's command sends its output to (>
// f, >> f, &> f, | tee f), a cd before them heeded: where its output is
// when Claude Code's own file has none.
func (s *Session) JobWrites(j *Job) []string { return s.Writes(s.JobCommand(j)) }

// Writes are the files a shell command sends its output to, as JobWrites.
func (s *Session) Writes(cmd string) []string {
	dir := s.Info.Cwd
	var out []string
	for _, sg := range segments(cmd) {
		if w := fieldsOf(sg.text); len(w) == 2 && w[0] == "cd" {
			dir = writePath(dir, w[1])
			continue
		}
		for _, stage := range append([]string{sg.text}, sg.filters...) {
			for _, re := range []*regexp.Regexp{writeRe, teeRe} {
				for _, m := range re.FindAllStringSubmatch(stage, -1) {
					if p := writePath(dir, m[1]); p != "" && !strings.HasPrefix(p, "/dev/") && !slices.Contains(out, p) {
						out = append(out, p)
					}
				}
			}
		}
	}
	return out
}

// writePath is p as written in a command run in dir, made absolute; "" when a
// variable makes it unknowable.
func writePath(dir, p string) string {
	p = unquote(p)
	switch {
	case strings.Contains(p, "$") || p == "":
		return ""
	case p == "~" || strings.HasPrefix(p, "~/"):
		home, _ := os.UserHomeDir()
		return filepath.Join(home, p[1:])
	case !filepath.IsAbs(p) && dir == "":
		return ""
	case !filepath.IsAbs(p):
		return filepath.Join(dir, p)
	}
	return filepath.Clean(p)
}
