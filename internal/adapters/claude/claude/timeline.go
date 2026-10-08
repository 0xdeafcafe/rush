package claude

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// A Timeline is what a session and its subagents have done, in time order:
// what you asked, tasks planned and ticked off, runs started and ended,
// turns finished, questions asked, commits made, PRs opened and turns that
// died on an error. With it, what each run last said and is doing.
//
// It reads only the tail of a transcript the first time (a day's work is
// rarely more), then only what each has gained.

// timelineTail is how much of a transcript is read the first time.
const timelineTail = 4 << 20

// timelineCap is how many events a session keeps, the newest.
const timelineCap = 600

type (
	EventKind = agent.EventKind
	Happening = agent.Happening
)

const (
	EvPrompt  = agent.EvPrompt
	EvTurn    = agent.EvTurn
	EvPlan    = agent.EvPlan
	EvTick    = agent.EvTick
	EvStart   = agent.EvStart
	EvEnd     = agent.EvEnd
	EvAsk     = agent.EvAsk
	EvCommit  = agent.EvCommit
	EvPR      = agent.EvPR
	EvError   = agent.EvError
	EvCompact = agent.EvCompact
)

// Run is a subagent as the timeline knows it.
type Run struct {
	ID, Name, Type string
	ToolUseID      string
	Depth          int
	Mod            time.Time // its transcript last written
	Said           string    // the last thing it wrote
	SaidAt         time.Time
	Doing          string // its last tool call, in words
	DoingAt        time.Time
	Ended          bool
	Status         string
	EndedAt        time.Time
	Ticked, Tasks  int
	// held is a report just handed back: what it writes after ("I've sent
	// the report") doesn't replace it.
	held bool
}

type Timeline struct {
	path     string
	files    map[string]*tlFile
	runs     map[string]*Run
	metas    map[string]time.Time // meta files read, by path: their mtime
	calls    map[string]string    // Agent calls' descriptions, by tool_use id
	notified map[string]bool
	events   []Happening
	buf      []byte
}

type tlFile struct {
	off     int64
	run     string
	tasks   map[string]string // task subjects, by id
	creates map[string]string // TaskCreate calls' subjects, by tool_use id
	todos   map[string]string // TodoWrite items' statuses, by content
}

// TimelineView is a copy of a timeline, safe to hold while it reads on.
type TimelineView struct {
	Events []Happening // oldest first
	Runs   []Run       // oldest first
}

// Update reads what the session's transcript at path, and its subagents',
// have gained. Subagents last written before since aren't read.
func (t *Timeline) Update(path string, since time.Time) {
	if t.path != path || t.files == nil {
		*t = Timeline{path: path, files: map[string]*tlFile{}, runs: map[string]*Run{}, metas: map[string]time.Time{},
			calls: map[string]string{}, notified: map[string]bool{}}
	}
	dir := filepath.Join(strings.TrimSuffix(path, ".jsonl"), "subagents")
	metas, _ := filepath.Glob(filepath.Join(dir, "agent-*.meta.json"))
	for _, p := range metas {
		id := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(p), "agent-"), ".meta.json")
		fi, err := os.Stat(filepath.Join(dir, "agent-"+id+".jsonl"))
		if err != nil || fi.ModTime().Before(since) {
			continue
		}
		r := t.runs[id]
		if r == nil {
			r = &Run{ID: id, Depth: 1}
			t.runs[id] = r
		}
		if mi, err := os.Stat(p); err == nil && !t.metas[p].Equal(mi.ModTime()) {
			t.metas[p] = mi.ModTime()
			var m struct {
				Type        string `json:"agentType"`
				Description string `json:"description"`
				ToolUse     string `json:"toolUseId"`
				Depth       int    `json:"spawnDepth"`
			}
			if b, err := os.ReadFile(p); err == nil && jsonx.Unmarshal(b, &m) == nil {
				r.Name, r.Type, r.ToolUseID, r.Depth = m.Description, m.Type, m.ToolUse, max(1, m.Depth)
			}
		}
		if r.Name == "" {
			r.Name = t.calls[r.ToolUseID]
		}
		r.Mod = fi.ModTime()
	}
	// The runs are read before the session: word of their ends comes in
	// its transcript, and says less than their own last words.
	for id := range t.runs {
		t.read(filepath.Join(dir, "agent-"+id+".jsonl"), id)
	}
	t.read(path, "")
	for _, r := range t.runs {
		if r.Name == "" {
			r.Name = t.calls[r.ToolUseID]
		}
	}
}

// View copies the timeline, dropping events before since.
func (t *Timeline) View(since time.Time) TimelineView {
	var v TimelineView
	for _, e := range t.events {
		if !e.At.Before(since) {
			v.Events = append(v.Events, e)
		}
	}
	sort.SliceStable(v.Events, func(i, j int) bool { return v.Events[i].At.Before(v.Events[j].At) })
	for _, r := range t.runs {
		v.Runs = append(v.Runs, *r)
	}
	sort.Slice(v.Runs, func(i, j int) bool { return v.Runs[i].ID < v.Runs[j].ID })
	sort.SliceStable(v.Runs, func(i, j int) bool { return v.Runs[i].started().Before(v.Runs[j].started()) })
	return v
}

// started is as near as a run knows to when it began: its first words.
func (r Run) started() time.Time {
	switch {
	case !r.SaidAt.IsZero() && (r.DoingAt.IsZero() || r.SaidAt.Before(r.DoingAt)):
		return r.SaidAt
	case !r.DoingAt.IsZero():
		return r.DoingAt
	}
	return r.Mod
}

func (t *Timeline) read(path, run string) {
	f, err := os.Open(path)
	if err != nil {
		return
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return
	}
	tf := t.files[path]
	skip := false // the tail starts mid-line
	if tf == nil || st.Size() < tf.off {
		tf = &tlFile{run: run, tasks: map[string]string{}, creates: map[string]string{}, todos: map[string]string{}}
		t.files[path] = tf
		if st.Size() > timelineTail {
			tf.off, skip = st.Size()-timelineTail, true
		}
	}
	if st.Size() == tf.off {
		return
	}
	if _, err := f.Seek(tf.off, io.SeekStart); err != nil {
		return
	}
	br := bufio.NewReaderSize(f, 64<<10)
	for {
		t.buf = t.buf[:0]
		complete := false
		for {
			chunk, err := br.ReadSlice('\n')
			t.buf = append(t.buf, chunk...)
			if err == nil {
				complete = true
				break
			}
			if err != bufio.ErrBufferFull {
				break
			}
		}
		if !complete {
			break
		}
		tf.off += int64(len(t.buf))
		if skip {
			skip = false
			continue
		}
		t.line(tf, t.buf)
	}
	if cap(t.buf) > 1<<20 {
		t.buf = nil
	}
	if n := len(t.events); n > timelineCap {
		t.events = append([]Happening(nil), t.events[n-timelineCap:]...)
	}
}

var (
	commitLine = regexp.MustCompile(`(?m)^\[[\w./-]+(?: \(root-commit\))? ([0-9a-f]{7,40})\] (.+)$`)
	prLine     = regexp.MustCompile(`(?m)^https://github\.com/[^\s/]+/[^\s/]+/pull/\d+\s*$`)
	notifySum  = regexp.MustCompile(`^Agent "(.*)" (finished|completed|failed|was stopped|stopped)(?:: (.*))?$`)
)

type tlLine struct {
	Type      string    `json:"type"`
	Subtype   string    `json:"subtype"`
	Timestamp time.Time `json:"timestamp"`
	IsMeta    bool      `json:"isMeta"`
	APIError  bool      `json:"isApiErrorMessage"`
	Error     string    `json:"error"`
	Message   struct {
		Content    jsontext.Value `json:"content"`
		StopReason string         `json:"stop_reason"`
	} `json:"message"`
	ToolUseResult jsontext.Value `json:"toolUseResult"`
}

type tlBlock struct {
	Type      string         `json:"type"`
	Text      string         `json:"text"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Input     jsontext.Value `json:"input"`
	ToolUseID string         `json:"tool_use_id"`
	Content   jsontext.Value `json:"content"`
}

var (
	userMark      = []byte(`"type":"user"`)
	assistantMark = []byte(`"type":"assistant"`)
	compactMark   = []byte(`"compact_boundary"`)
	// A user line that is a tool's result matters only for these.
	resultMarks = [][]byte{[]byte(`"statusChange"`), []byte(`"task":{`), []byte(`/pull/`), notifyOpen, []byte(`] `)}
)

func (t *Timeline) line(tf *tlFile, b []byte) {
	user, asst := bytes.Contains(b, userMark), bytes.Contains(b, assistantMark)
	compact := bytes.Contains(b, compactMark)
	if !user && !asst && !compact {
		return
	}
	if user && bytes.Contains(b, toolResultMark) {
		hit := false
		for _, m := range resultMarks {
			if bytes.Contains(b, m) {
				hit = true
				break
			}
		}
		if !hit {
			return
		}
	}
	var l tlLine
	if jsonx.Unmarshal(b, &l) != nil {
		return
	}
	at := l.Timestamp
	add := func(e Happening) {
		e.At, e.Run = at, tf.run
		t.events = append(t.events, e)
	}
	switch {
	case l.Type == "system" && l.Subtype == "compact_boundary":
		add(Happening{Kind: EvCompact})
	case l.Type == "assistant":
		t.assistant(tf, &l, add)
	case l.Type == "user" && !l.IsMeta:
		t.user(tf, &l, add)
	}
}

func (t *Timeline) assistant(tf *tlFile, l *tlLine, add func(Happening)) {
	var blocks []tlBlock
	_ = jsonx.Unmarshal(l.Message.Content, &blocks)
	r := t.runs[tf.run]
	text := ""
	for _, bl := range blocks {
		switch bl.Type {
		case "text":
			if s := strings.TrimSpace(bl.Text); s != "" {
				text = s
			}
		case "tool_use":
			t.toolUse(tf, bl, add)
			if r != nil && bl.Name != "" {
				r.Doing, r.DoingAt = Doing(bl.Name, bl.Input), l.Timestamp
				r.held = false
				if bl.Name == "SubagentHandback" || bl.Name == "SendMessage" {
					var in struct{ Message string }
					if jsonx.Unmarshal(bl.Input, &in) == nil && in.Message != "" {
						r.Said, r.SaidAt, r.held = said(in.Message), l.Timestamp, true
					}
				}
			}
		}
	}
	if l.APIError {
		if l.Error != "" {
			add(Happening{Kind: EvError, Text: strings.TrimPrefix(text, "API Error: ")})
		}
		return
	}
	if text == "" {
		return
	}
	text = said(text)
	if text == "No response requested." {
		return
	}
	if r != nil {
		if r.held {
			text = r.Said
		} else {
			r.Said, r.SaidAt = text, l.Timestamp
		}
	}
	if l.Message.StopReason != "end_turn" {
		return
	}
	if r == nil {
		add(Happening{Kind: EvTurn, Text: text})
		return
	}
	// A run's turn ending is the run ending, unless a message wakes it.
	if !r.Ended || r.EndedAt.Before(l.Timestamp) {
		r.Ended, r.Status, r.EndedAt = true, "completed", l.Timestamp
		add(Happening{Kind: EvEnd, Status: "completed", Text: text})
	}
}

func (t *Timeline) toolUse(tf *tlFile, bl tlBlock, add func(Happening)) {
	switch bl.Name {
	case "TaskCreate":
		var in struct{ Subject string }
		if jsonx.Unmarshal(bl.Input, &in) == nil {
			tf.creates[bl.ID] = in.Subject
		}
	case "TaskUpdate":
		var in struct {
			TaskID string `json:"taskId"`
			Status string `json:"status"`
		}
		if jsonx.Unmarshal(bl.Input, &in) == nil && in.Status == "completed" {
			s := tf.tasks[in.TaskID]
			if s == "" {
				s = "task #" + in.TaskID
			}
			add(Happening{Kind: EvTick, Text: s})
			if r := t.runs[tf.run]; r != nil {
				r.Ticked++
			}
		}
	case "TodoWrite":
		var in struct {
			Todos []struct{ Content, Status string } `json:"todos"`
		}
		if jsonx.Unmarshal(bl.Input, &in) != nil {
			return
		}
		fresh := len(tf.todos) == 0
		next := map[string]string{}
		for _, td := range in.Todos {
			next[td.Content] = td.Status
			if td.Status == "completed" && tf.todos[td.Content] != "completed" && !fresh {
				add(Happening{Kind: EvTick, Text: td.Content})
			}
		}
		if fresh && len(in.Todos) > 0 {
			add(Happening{Kind: EvPlan, N: len(in.Todos), Text: in.Todos[0].Content})
		}
		tf.todos = next
	case "Agent", "Task":
		var in struct{ Description string }
		if jsonx.Unmarshal(bl.Input, &in) == nil {
			t.calls[bl.ID] = in.Description
			add(Happening{Kind: EvStart, Text: in.Description})
		}
	case "AskUserQuestion":
		var in struct {
			Questions []struct{ Question string } `json:"questions"`
		}
		if jsonx.Unmarshal(bl.Input, &in) == nil && len(in.Questions) > 0 {
			add(Happening{Kind: EvAsk, Text: in.Questions[0].Question})
		}
	}
}

func (t *Timeline) user(tf *tlFile, l *tlLine, add func(Happening)) {
	var s string
	var blocks []tlBlock
	if jsonx.Unmarshal(l.Message.Content, &s) != nil {
		_ = jsonx.Unmarshal(l.Message.Content, &blocks)
	}
	for _, bl := range blocks {
		switch bl.Type {
		case "text":
			s += bl.Text
		case "tool_result":
			t.toolResult(tf, bl, l, add)
		}
	}
	s = strings.TrimSpace(s)
	switch {
	case s == "":
	case strings.Contains(s, "<task-notification>"):
		t.notification(s, add)
	case tf.run != "" || strings.HasPrefix(s, "<") || strings.HasPrefix(s, "[Request interrupted"):
		// a run's brief, or a command's echo, rather than you
	default:
		add(Happening{Kind: EvPrompt, Text: said(s)})
	}
}

func (t *Timeline) toolResult(tf *tlFile, bl tlBlock, l *tlLine, add func(Happening)) {
	if subj, ok := tf.creates[bl.ToolUseID]; ok {
		delete(tf.creates, bl.ToolUseID)
		var res struct {
			Task struct{ ID, Subject string } `json:"task"`
		}
		if jsonx.Unmarshal(l.ToolUseResult, &res) == nil && res.Task.ID != "" {
			if res.Task.Subject != "" {
				subj = res.Task.Subject
			}
			tf.tasks[res.Task.ID] = subj
		}
		if r := t.runs[tf.run]; r != nil {
			r.Tasks++
		}
		// Tasks are planned in a burst: one event says how many.
		if n := len(t.events); n > 0 {
			if e := &t.events[n-1]; e.Kind == EvPlan && e.Run == tf.run && l.Timestamp.Sub(e.At) < 5*time.Minute {
				e.N++
				return
			}
		}
		add(Happening{Kind: EvPlan, N: 1, Text: subj})
		return
	}
	var text string
	if jsonx.Unmarshal(bl.Content, &text) != nil {
		var parts []tlBlock
		_ = jsonx.Unmarshal(bl.Content, &parts)
		for _, p := range parts {
			text += p.Text
		}
	}
	if m := commitLine.FindStringSubmatch(text); m != nil {
		add(Happening{Kind: EvCommit, Text: m[1][:7] + " " + m[2]})
	}
	if u := prLine.FindString(text); u != "" && len(text) < 400 {
		add(Happening{Kind: EvPR, Text: strings.TrimSpace(u)})
	}
	if strings.Contains(text, "<task-notification>") {
		t.notification(text, add)
	}
}

// notification is word that a run, or a background command, ended.
func (t *Timeline) notification(s string, add func(Happening)) {
	for rest := s; ; {
		i := strings.Index(rest, "<task-notification>")
		if i < 0 {
			return
		}
		rest = rest[i+len("<task-notification>"):]
		seg := rest
		if j := strings.Index(rest, "</task-notification>"); j >= 0 {
			seg = rest[:j]
		}
		b := []byte(seg)
		status := tagFirst(tagTexts(b, "status"))
		if status == "" {
			continue
		}
		switch status {
		case "completed", "failed", "stopped":
		case "killed":
			status = "stopped"
		default:
			continue
		}
		ids := append(tagTexts(b, "task-id"), tagTexts(b, "tool-use-id")...)
		key := strings.Join(ids, ",") + status
		if t.notified[key] {
			continue // the same notice, queued then delivered
		}
		t.notified[key] = true
		sum := tagFirst(tagTexts(b, "summary"))
		res := tagFirst(tagTexts(b, "result"))
		var run *Run
		for _, r := range t.runs {
			for _, id := range ids {
				if id != "" && (id == r.ID || id == r.ToolUseID) {
					run = r
				}
			}
		}
		text := sum
		if m := notifySum.FindStringSubmatch(sum); m != nil {
			text = m[3] // the name is the run's own
			if text == "" && res != "" {
				text = said(res)
			}
		}
		e := Happening{Kind: EvEnd, Status: status, Text: text}
		if run != nil {
			if run.Ended && run.Status == status {
				continue // its own transcript said so already
			}
			run.Ended, run.Status = true, status
			if status == "completed" && run.Said != "" {
				e.Text = run.Said
			}
			n := len(t.events)
			add(e)
			t.events[n].Run = run.ID
			run.EndedAt = t.events[n].At
			continue
		}
		if m := notifySum.FindStringSubmatch(sum); m != nil && e.Text == "" {
			e.Text = m[1]
		} else if m != nil {
			e.Text = m[1] + ": " + e.Text
		}
		add(e)
	}
}

func tagFirst(s []string) string {
	if len(s) == 0 {
		return ""
	}
	return s[0]
}

// said is the gist of something written: its first paragraph, without
// markdown's marks, and no longer than a line could show.
func said(s string) string {
	s = strings.TrimSpace(s)
	if p, rest, ok := strings.Cut(s, "\n\n"); ok {
		s = p
		if strings.HasSuffix(p, ":") {
			// a lead-in says nothing without what it leads to
			next, _, _ := strings.Cut(strings.TrimSpace(rest), "\n\n")
			s = p + " " + next
		}
	}
	s = strings.NewReplacer("**", "", "`", "", "\n", " ").Replace(s)
	s = strings.TrimLeft(s, "#> -")
	if r := []rune(s); len(r) > 300 {
		s = string(r[:300]) + "…"
	}
	return strings.TrimSpace(s)
}
