package claude

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"io"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// A subagent run's transcript says little about whether it is still going:
// one busy with a long command, or a long think, writes nothing for
// minutes. Its parent's transcript says more. The Agent (or Task) call
// that started it has no result while a run the turn waits on works; one
// launched in the background has a result at once, and a task
// notification when it finishes. SubagentRuns follows those.

// RunStale is how long a run the transcripts still call running may go
// without writing before it's taken to have died with its process.
const RunStale = 15 * time.Minute

// runQuiet is how recently a run the transcripts say nothing about must
// have written to count as running.
const runQuiet = 90 * time.Second

// RunState is how a session's transcripts say a subagent run stands.
type RunState = agent.RunState

const (
	RunUnknown = agent.RunUnknown
	RunRunning = agent.RunRunning
	RunDone    = agent.RunDone
)

// SubagentRuns follows a session's transcript, and its runs' own when one
// run started another, reading only what each has gained since.
type SubagentRuns struct {
	// Gone says the session's own process is known to have exited: a run
	// the transcripts left unfinished ended with it, at once, rather than
	// after RunStale. Left false when that can't be told.
	Gone bool

	path   string
	files  map[string]int64      // how far each transcript has been read
	calls  map[string]*agentCall // Agent calls, by tool_use id
	ends   map[string]runEnd     // finished runs, by agent id and by tool_use id
	woken  map[string]int        // runs a message was sent to, by agent id: when
	seq    int                   // lines read, which orders what they say
	unmet  int                   // calls still without their result: only then are results looked at
	metas  map[string]runMeta    // by meta file
	dirMod time.Time
	names  []string // the meta files as of dirMod
	listed time.Time
	buf    []byte

	// running are the runs Stats last found still working, for Running.
	running []SubagentRun
}

type agentCall struct {
	seq    int    // when it was made
	resSeq int    // when its result came
	result bool   // it has its result
	async  bool   // the result was the launch of a background run
	status string // how a run the turn waited on ended
	at     time.Time
}

type runEnd struct {
	status string
	at     time.Time
	seq    int
}

type runMeta struct {
	mod         time.Time
	size        int64
	id          string
	toolUse     string
	depth       int
	agentType   string
	description string
	// runMod is when the run's transcript was last written, as last
	// looked; runSeen is whether it has been.
	runMod  time.Time
	runSeen bool
}

// SubagentRun is one run as SubagentRuns knows it.
type SubagentRun = agent.SubagentRun

func (r *SubagentRuns) reset(path string) {
	*r = SubagentRuns{Gone: r.Gone, path: path, files: map[string]int64{}, calls: map[string]*agentCall{},
		ends: map[string]runEnd{}, woken: map[string]int{}, metas: map[string]runMeta{}}
}

// Update reads what the session's transcript at path, and its runs' when
// needed, have gained, and returns its runs.
func (r *SubagentRuns) Update(path string) []SubagentRun {
	if r.files == nil || r.path != path {
		r.reset(path)
	}
	runs := r.list()
	if len(runs) == 0 {
		return nil
	}
	r.readFor(path, runs)
	return runs
}

// UpdateRuns is Update for runs listed, from their meta files, by the
// caller: they aren't listed, nor their transcripts looked at, again.
func (r *SubagentRuns) UpdateRuns(path string, runs []SubagentRun) {
	if r.files == nil || r.path != path {
		r.reset(path)
	}
	if len(runs) > 0 {
		r.readFor(path, runs)
	}
}

// Took reads line, the next of the session's transcript at path, as read
// by another from its start: Update goes on from after it.
func (r *SubagentRuns) Took(path string, line []byte) {
	if r.files == nil || r.path != path {
		r.reset(path)
	}
	r.files[path] += int64(len(line))
	r.line(line)
}

// readFor reads what the session's transcript, and its runs' when needed,
// have gained.
func (r *SubagentRuns) readFor(path string, runs []SubagentRun) {
	r.read(path)
	// A run a run started has its call, and word of its end, in that run's
	// transcript: those are read only while such a run hasn't ended, and
	// only those that were going as it began.
	var kids []SubagentRun
	for _, x := range runs {
		if x.Depth > 1 {
			if st, _, _ := r.State(x.ID, x.ToolUseID); st != RunDone {
				kids = append(kids, x)
			}
		}
	}
	dir := r.dir()
	for _, x := range runs {
		if slices.ContainsFunc(kids, func(k SubagentRun) bool { return k.Depth == x.Depth+1 && mayHaveStarted(x, k) }) {
			r.read(filepath.Join(dir, "agent-"+x.ID+".jsonl"))
		}
	}
}

// mayHaveStarted is whether run p could have started run k: it had begun,
// and was still writing, as k began. Unknown times say it could.
func mayHaveStarted(p, k SubagentRun) bool {
	began := func(x SubagentRun) time.Time { // by its meta, or its writing if that's earlier
		if !x.Mod.IsZero() && x.Mod.Before(x.Born) {
			return x.Mod
		}
		return x.Born
	}
	kb, pb := began(k), began(p)
	return kb.IsZero() || (pb.IsZero() || !pb.After(kb)) && (p.Mod.IsZero() || !p.Mod.Before(kb.Add(-time.Minute)))
}

// Clone is a copy that answers State and Going as r does now, for the
// UI to ask while r goes on reading elsewhere.
func (r *SubagentRuns) Clone() SubagentRuns {
	c := SubagentRuns{Gone: r.Gone, path: r.path, seq: r.seq,
		calls: make(map[string]*agentCall, len(r.calls)), ends: maps.Clone(r.ends), woken: maps.Clone(r.woken)}
	for k, v := range r.calls {
		cp := *v
		c.calls[k] = &cp
	}
	return c
}

func (r *SubagentRuns) dir() string {
	return filepath.Join(strings.TrimSuffix(r.path, ".jsonl"), "subagents")
}

// list finds the runs from their meta files, reading each only when it's
// new or changed, and the folder only when something was added to it (or
// every 10s).
func (r *SubagentRuns) list() []SubagentRun {
	dir := r.dir()
	st, err := os.Stat(dir)
	if err != nil {
		r.names = nil
		return nil
	}
	fresh := st.ModTime().Equal(r.dirMod) && time.Since(r.listed) <= 10*time.Second
	if !fresh {
		r.names, _ = filepath.Glob(filepath.Join(dir, "agent-*.meta.json"))
		r.dirMod, r.listed = st.ModTime(), time.Now()
	}
	out := make([]SubagentRun, 0, len(r.names))
	for _, p := range r.names {
		m, ok := r.metas[p]
		// A meta file is written once, as its run starts: one read whole
		// is looked at again only with the folder, every 10s.
		if !ok || !fresh || m.toolUse == "" {
			fi, err := os.Stat(p)
			if err != nil {
				continue
			}
			if !ok || !m.mod.Equal(fi.ModTime()) || m.size != fi.Size() {
				m = runMeta{mod: fi.ModTime(), size: fi.Size(), depth: 1,
					id: strings.TrimSuffix(strings.TrimPrefix(filepath.Base(p), "agent-"), ".meta.json")}
				var v struct {
					ToolUse     string `json:"toolUseId"`
					Depth       int    `json:"spawnDepth"`
					AgentType   string `json:"agentType"`
					Description string `json:"description"`
				}
				if b, err := os.ReadFile(p); err == nil && jsonx.Unmarshal(b, &v) == nil {
					m.toolUse = v.ToolUse
					m.depth = max(1, v.Depth)
					m.agentType = v.AgentType
					m.description = v.Description
				}
				r.metas[p] = m
			}
		}
		run := SubagentRun{ID: m.id, ToolUseID: m.toolUse, Depth: m.depth, Type: m.agentType, Description: m.description, Born: m.mod}
		run.Path = filepath.Join(dir, "agent-"+m.id+".jsonl")
		// A run quiet past RunStale is looked at again only with the
		// folder, every 10s: a session can have hundreds, and each stat
		// on every reading was most of what reading the fleet cost.
		if !fresh || !m.runSeen || time.Since(m.runMod) < RunStale {
			m.runMod, m.runSeen = time.Time{}, true
			if fi, err := os.Stat(run.Path); err == nil {
				m.runMod = fi.ModTime()
			}
			r.metas[p] = m
		}
		run.Mod = m.runMod
		out = append(out, run)
	}
	return out
}

// read takes in the complete lines a transcript has gained.
func (r *SubagentRuns) read(path string) {
	f, err := os.Open(path)
	if err != nil {
		return
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return
	}
	off := r.files[path]
	if st.Size() < off {
		off = 0 // rewritten
	}
	if st.Size() == off {
		return
	}
	if _, err := f.Seek(off, io.SeekStart); err != nil {
		return
	}
	br := bufio.NewReaderSize(f, 64<<10)
	for {
		r.buf = r.buf[:0]
		complete := false
		for {
			chunk, err := br.ReadSlice('\n')
			r.buf = append(r.buf, chunk...)
			if err == nil {
				complete = true
				break
			}
			if err != bufio.ErrBufferFull {
				break
			}
		}
		if !complete {
			break // a line still being written is read whole next time
		}
		off += int64(len(r.buf))
		r.line(r.buf)
	}
	r.files[path] = off
	if cap(r.buf) > 1<<20 {
		r.buf = nil
	}
}

var (
	toolUseMark    = []byte(`"tool_use"`)
	agentNameMarks = [][]byte{[]byte(`"name":"Agent"`), []byte(`"name":"Task"`), []byte(`"name":"SendMessage"`)}
	toolResultMark = []byte(`"tool_result"`)
	notifyOpen     = []byte("<task-notification>")
	notifyClose    = []byte("</task-notification>")
	asyncMarks     = [][]byte{[]byte(`"async_launched"`), []byte("Async agent launched")}
	stampMark      = []byte(`"timestamp":"`)
)

func (r *SubagentRuns) line(b []byte) {
	r.seq++
	if bytes.Contains(b, toolUseMark) && (bytes.Contains(b, agentNameMarks[0]) || bytes.Contains(b, agentNameMarks[1]) || bytes.Contains(b, agentNameMarks[2])) {
		for _, bl := range lineBlocks(b) {
			switch {
			case bl.Type != "tool_use":
			case (bl.Name == "Agent" || bl.Name == "Task") && r.calls[bl.ID] == nil:
				r.calls[bl.ID] = &agentCall{seq: r.seq}
				r.unmet++
			case bl.Name == "SendMessage":
				// A message to a finished run wakes it, until it next ends.
				if bl.Input != "" {
					r.woken[string(bl.Input)] = r.seq
				}
			}
		}
	}
	if r.unmet > 0 && bytes.Contains(b, toolResultMark) {
		for _, bl := range lineBlocks(b) {
			c := r.calls[bl.ToolUseID]
			if bl.Type != "tool_result" || c == nil || c.result {
				continue
			}
			c.result, c.at, c.resSeq = true, lineTime(b), r.seq
			r.unmet--
			c.async = bytes.Contains(b, asyncMarks[0]) || bytes.Contains(b, asyncMarks[1])
			c.status = "completed"
			switch {
			case bool(bl.Content):
				c.status = "stopped"
			case bl.IsError:
				c.status = "failed"
			}
		}
	}
	for rest := b; ; {
		i := bytes.Index(rest, notifyOpen)
		if i < 0 {
			break
		}
		rest = rest[i+len(notifyOpen):]
		seg := rest
		if j := bytes.Index(rest, notifyClose); j >= 0 {
			seg = rest[:j]
		}
		status := tagTexts(seg, "status")
		if len(status) == 0 {
			continue
		}
		end := runEnd{status: status[0], at: lineTime(b), seq: r.seq}
		if end.status == "killed" {
			end.status = "stopped"
		}
		// One notice can tell of several runs ending (a session that
		// ended with them running); the same one comes again as it's
		// queued and delivered, which changes nothing.
		for _, id := range append(tagTexts(seg, "task-id"), tagTexts(seg, "tool-use-id")...) {
			if e, seen := r.ends[id]; !seen || e.status != end.status || r.woken[id] > e.seq {
				r.ends[id] = end
			}
		}
	}
}

type lineBlock struct {
	Type      string      `json:"type"`
	ID        string      `json:"id"`
	Name      string      `json:"name"`
	ToolUseID string      `json:"tool_use_id"`
	IsError   bool        `json:"is_error"`
	Input     sendTo      `json:"input"`
	Content   interrupted `json:"content"`
}

// A block's input and content can be a whole prompt or a tool's whole
// output, and a run's lines are only asked two things of them. Each is
// looked at where the decoder holds it, never copied.

// sendTo is a SendMessage call's input: who it's to.
type sendTo string

func (s *sendTo) UnmarshalJSONFrom(d *jsontext.Decoder) error {
	v, err := d.ReadValue()
	if err != nil || !bytes.Contains(v, toMark) {
		return err
	}
	var in struct {
		To string `json:"to"`
	}
	if jsonx.Unmarshal(v, &in) == nil {
		*s = sendTo(in.To)
	}
	return nil
}

// interrupted is a tool result's content: whether it says the request
// was interrupted.
type interrupted bool

func (x *interrupted) UnmarshalJSONFrom(d *jsontext.Decoder) error {
	v, err := d.ReadValue()
	*x = interrupted(err == nil && bytes.Contains(v, interruptMark))
	return err
}

// lineContent is a message's content: its blocks, or none for one
// written as plain text.
type lineContent []lineBlock

func (c *lineContent) UnmarshalJSONFrom(d *jsontext.Decoder) error {
	if d.PeekKind() != '[' {
		return d.SkipValue()
	}
	var bl []lineBlock
	err := jsonx.DecodeValue(d, &bl)
	*c = bl
	return err
}

var (
	toMark        = []byte(`"to"`)
	interruptMark = []byte("[Request interrupted")
)

// lineBlocks are a transcript line's message's content blocks.
func lineBlocks(b []byte) []lineBlock {
	var l struct {
		Message struct {
			Content lineContent `json:"content"`
		} `json:"message"`
	}
	if jsonx.Unmarshal(b, &l) != nil {
		return nil
	}
	return l.Message.Content
}

// lineTime is a transcript line's time, or now.
func lineTime(b []byte) time.Time {
	if i := bytes.Index(b, stampMark); i >= 0 {
		s := b[i+len(stampMark):]
		if j := bytes.IndexByte(s, '"'); j > 0 {
			if t, err := time.Parse(time.RFC3339Nano, string(s[:j])); err == nil {
				return t
			}
		}
	}
	return time.Now()
}

// tagTexts is what's inside each <tag>…</tag> in s.
func tagTexts(s []byte, tag string) []string {
	open, end := []byte("<"+tag+">"), []byte("</"+tag+">")
	var out []string
	for {
		i := bytes.Index(s, open)
		if i < 0 {
			return out
		}
		s = s[i+len(open):]
		j := bytes.Index(s, end)
		if j < 0 {
			return out
		}
		if t := strings.TrimSpace(string(s[:j])); t != "" {
			out = append(out, t)
		}
		s = s[j+len(end):]
	}
}

// State is how the run with this agent id, started by this call, stands:
// and when it has ended, how and when.
// Whatever the transcripts said last counts: a run launched, or woken by a
// message, runs until a notice says it ended.
func (r *SubagentRuns) State(id, toolUseID string) (RunState, string, time.Time) {
	run, done := -1, -1
	var end runEnd
	if c := r.calls[toolUseID]; c != nil && toolUseID != "" {
		switch {
		case !c.result:
			run = c.seq
		case c.async:
			run = c.resSeq
		default:
			done, end = c.resSeq, runEnd{status: c.status, at: c.at}
		}
	}
	for _, k := range []string{id, toolUseID} {
		if e, ok := r.ends[k]; ok && k != "" && e.seq > done {
			done, end = e.seq, e
		}
	}
	if w, ok := r.woken[id]; ok && id != "" && w > run {
		run = w
	}
	switch {
	case run < 0 && done < 0:
		return RunUnknown, "", time.Time{}
	case run > done:
		return RunRunning, "", time.Time{}
	}
	return RunDone, end.status, end.at
}

// Going is whether a run is still working, last written at mod, and how it
// ended if it has. The transcripts' word counts: a run they call running
// is, unless its session's process is Gone ("ended"), or, when that can't
// be told, it's been silent past RunStale (it died with its process);
// one they call done is, unless it's written since (a message sent to it
// woke it). With no word, it's running while it writes.
func (r *SubagentRuns) Going(id, toolUseID string, mod, now time.Time) (bool, string) {
	st, status, at := r.State(id, toolUseID)
	recent := !mod.IsZero() && now.Sub(mod) < runQuiet
	switch st {
	case RunRunning:
		if r.Gone {
			return false, "ended"
		}
		return mod.IsZero() || now.Sub(mod) < RunStale, ""
	case RunDone:
		if recent && mod.After(at.Add(5*time.Second)) {
			return true, ""
		}
		return false, status
	}
	return recent, ""
}

// Stats counts the session's runs, and those still working, directly and
// at any depth.
func (r *SubagentRuns) Stats(path string, now time.Time) SubagentStats {
	if r.files == nil || r.path != path {
		r.reset(path)
	}
	runs := r.list()
	st := SubagentStats{Spawned: len(runs)}
	// Only a run written in the last RunStale can be working: when none
	// has been, what the transcripts say needn't be read. An old session's
	// can be tens of megabytes, and every one was read as rush started.
	if !slices.ContainsFunc(runs, func(x SubagentRun) bool { return x.Mod.IsZero() || now.Sub(x.Mod) < RunStale }) {
		r.running = nil
		return st
	}
	r.readFor(path, runs)
	r.running = r.running[:0]
	for _, x := range runs {
		if going, _ := r.Going(x.ID, x.ToolUseID, x.Mod, now); !going {
			continue
		}
		if x.Depth <= 1 {
			st.Direct++
		} else {
			st.Nested++
		}
		r.running = append(r.running, x)
	}
	return st
}

// Running are the runs Stats last found still working: for drawing each as
// its own tile rather than behind the count alone.
func (r *SubagentRuns) Running() []SubagentRun { return r.running }
