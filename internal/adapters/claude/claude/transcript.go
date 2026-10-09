package claude

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Totals is what one transcript file has cost so far. It is small and
// serialisable so the scan resumes from Offset instead of re-reading.
type Totals struct {
	Offset    int64                  `json:"o"`
	Size      int64                  `json:"s"`
	ByModel   map[string]*TokenUsage `json:"m,omitempty"`
	Cost      float64                `json:"c"`
	First     time.Time              `json:"f"`
	Last      time.Time              `json:"l"`
	PRs       []string               `json:"p,omitempty"`
	LastModel string                 `json:"lm,omitempty"`
	Days      map[string]float64     `json:"d,omitempty"`
	// Dirs are the folders the session worked in (the line's cwd), which
	// says which worktrees are its; capped so a wandering one stays small.
	Dirs []string `json:"w,omitempty"`
	// Dir is where the session works now: where it last wrote a file, or,
	// before it has, its newest cwd or where it last cd'd, since Claude
	// Code's cwd never follows it into a checkout outside the one it started
	// in. Once it writes, only another write or entering a worktree moves
	// it: a session editing a worktree that runs git from the main checkout
	// would otherwise flip between the two with every command.
	Dir string `json:"cw,omitempty"`
	// Wrote says Dir is where it last wrote.
	Wrote bool `json:"ww,omitzero"`
	// DirAt is when Dir last changed: a host that moved the session since
	// knows better.
	DirAt time.Time `json:"wa,omitzero"`
	// Cwd is the newest line's cwd, so only a real move of it overrides Dir.
	Cwd string `json:"cc,omitempty"`
	// pending is the newest assistant message; its usage can still change
	// while more content blocks of the same message are appended.
	PendingID    string     `json:"pi,omitempty"`
	PendingModel string     `json:"pm,omitempty"`
	PendingUse   TokenUsage `json:"pu"`
	PendingFast  bool       `json:"pf,omitzero"`
	PendingAt    time.Time  `json:"pa"`
	// Halt is the error the session's last turn ended on, if it ended on
	// one; the next real reply clears it.
	Halt *Halt `json:"h,omitempty"`
	// Progress is the last sentence it wrote with a count going from one
	// number to another ("lint 11,065 → 9,052"), and when.
	Progress   string    `json:"g,omitempty"`
	ProgressAt time.Time `json:"ga"`
	// Compacts is how many times its context was compacted.
	Compacts int `json:"k,omitzero"`
}

// Context is how much of the model's window the newest message used.
func (t *Totals) Context() int64 {
	u := t.PendingUse
	return u.Input + u.CacheRead + u.CacheWrite5m + u.CacheWrite1h
}

// Halt is a turn Claude Code ended on an error instead of an answer; its
// Kind is Claude Code's: rate_limit, server_error, authentication_failed…
type Halt = agent.Halt

func Day(t time.Time) string { return t.Local().Format("2006-01-02") }

func (t *Totals) commitPending() {
	if t.PendingID == "" {
		return
	}
	if t.ByModel == nil {
		t.ByModel = map[string]*TokenUsage{}
	}
	u := t.ByModel[t.PendingModel]
	if u == nil {
		u = &TokenUsage{}
		t.ByModel[t.PendingModel] = u
	}
	u.Add(t.PendingUse)
	c := Cost(t.PendingModel, t.PendingUse, t.PendingFast)
	t.Cost += c
	if t.Days == nil {
		t.Days = map[string]float64{}
	}
	t.Days[Day(t.PendingAt)] += c
	t.PendingID, t.PendingModel, t.PendingUse, t.PendingFast = "", "", TokenUsage{}, false
}

// Spend includes the pending message so a running agent's figure moves live.
func (t *Totals) Spend() (float64, TokenUsage) {
	var u TokenUsage
	for _, m := range t.ByModel {
		u.Add(*m)
	}
	u.Add(t.PendingUse)
	return t.Cost + Cost(t.PendingModel, t.PendingUse, t.PendingFast), u
}

// DaySpend is what was spent on one local calendar day.
func (t *Totals) DaySpend(day string) float64 {
	c := t.Days[day]
	if t.PendingID != "" && Day(t.PendingAt) == day {
		c += Cost(t.PendingModel, t.PendingUse, t.PendingFast)
	}
	return c
}

type Preview = agent.Preview

type Event = agent.Line

// ContextWindow is the model's window; everything current but Haiku has 1M.
func ContextWindow(model string) int64 {
	if strings.Contains(model, "haiku") {
		return 200_000
	}
	return 1_000_000
}

type line struct {
	Type      string    `json:"type"`
	Timestamp time.Time `json:"timestamp"`
	Cwd       string    `json:"cwd"`
	Effort    string    `json:"effort"`
	APIError  bool      `json:"isApiErrorMessage"`
	Error     string    `json:"error"`
	Message   struct {
		ID      string         `json:"id"`
		Model   string         `json:"model"`
		Role    string         `json:"role"`
		Content jsontext.Value `json:"content"`
		Usage   *struct {
			Input        int64  `json:"input_tokens"`
			Output       int64  `json:"output_tokens"`
			CacheRead    int64  `json:"cache_read_input_tokens"`
			CacheCreate  int64  `json:"cache_creation_input_tokens"`
			Speed        string `json:"speed"`
			CacheBreakup *struct {
				M5 int64 `json:"ephemeral_5m_input_tokens"`
				H1 int64 `json:"ephemeral_1h_input_tokens"`
			} `json:"cache_creation"`
		} `json:"usage"`
	} `json:"message"`
}

var (
	assistantMarker = []byte(`"type":"assistant"`)
	compactMarker   = []byte(`"compact_boundary"`)
	relocatedMarker = []byte(`"type":"relocated"`)
	arrowMarkers    = [][]byte{[]byte("→"), []byte("->")}
	// progressPair is a count going somewhere: "11,065 → 9,052", "22->18".
	progressPair = regexp.MustCompile(`((?:\d[\d,.]*)?\dk?)\**\s?(?:→|->)\s?\**~?((?:\d[\d,.]*)?\dk?)`)
	prURL        = regexp.MustCompile(`https://github\.com/[\w.-]+/[\w.-]+/pull/\d+`)
)

// scanReaders are Scan's readers, kept: a scan every few seconds of the
// few transcripts that grew would otherwise make a 256 KB buffer each.
var scanReaders = sync.Pool{New: func() any { return bufio.NewReaderSize(nil, 256<<10) }}

// Scan advances t over whatever was appended to path since t.Offset. Only
// complete lines are consumed, so a half-written line is read next time.
func Scan(path string, t *Totals, buf []byte) ([]byte, error) {
	f, err := os.Open(path)
	if err != nil {
		return buf, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return buf, err
	}
	if st.Size() < t.Offset {
		*t = Totals{}
	}
	t.Size = st.Size()
	if st.Size() == t.Offset {
		return buf, nil
	}
	if _, err := f.Seek(t.Offset, io.SeekStart); err != nil {
		return buf, err
	}
	r := scanReaders.Get().(*bufio.Reader)
	r.Reset(f)
	defer func() { r.Reset(nil); scanReaders.Put(r) }()
	for {
		buf = buf[:0]
		complete := false
		for {
			chunk, err := r.ReadSlice('\n')
			buf = append(buf, chunk...)
			if err == nil {
				complete = true
				break
			}
			if err != bufio.ErrBufferFull {
				break
			}
		}
		if !complete {
			return buf, nil
		}
		t.Offset += int64(len(buf))
		consume(t, buf)
	}
}

func consume(t *Totals, b []byte) {
	if bytes.Contains(b, compactMarker) && bytes.Contains(b, []byte(`"subtype":"compact_boundary"`)) {
		t.Compacts++
		return
	}
	// Entering or leaving a worktree moves the transcript and says where:
	// the session works there from now on, before its next reply says so.
	if bytes.Contains(b, relocatedMarker) {
		var r struct {
			Cwd       string    `json:"relocatedCwd"`
			Timestamp time.Time `json:"timestamp"`
		}
		if jsonx.Unmarshal(b, &r) == nil && r.Cwd != "" {
			t.Cwd, t.Wrote = r.Cwd, false
			t.setDir(r.Cwd, r.Timestamp)
		}
		return
	}
	if !bytes.Contains(b, assistantMarker) {
		return
	}
	var l line
	if jsonx.Unmarshal(b, &l) != nil || l.Type != "assistant" {
		return
	}
	if l.Cwd != "" {
		if l.Cwd != t.Cwd {
			t.Cwd = l.Cwd
			if !t.Wrote {
				t.setDir(l.Cwd, l.Timestamp)
			}
		}
		if len(t.Dirs) < 64 && (len(t.Dirs) == 0 || t.Dirs[len(t.Dirs)-1] != l.Cwd) {
			addUnique(&t.Dirs, l.Cwd)
		}
	}
	if bytes.Contains(b, toolUseMarker) {
		if d, wrote := workedIn(l.Message.Content); d != "" && (wrote || !t.Wrote) {
			t.Wrote = wrote
			t.setDir(d, l.Timestamp)
			if len(t.Dirs) < 64 {
				addUnique(&t.Dirs, d)
			}
		}
	}
	if !l.Timestamp.IsZero() {
		if t.First.IsZero() {
			t.First = l.Timestamp
		}
		t.Last = l.Timestamp
	}
	if bytes.Contains(b, []byte("/pull/")) {
		for _, m := range prURL.FindAll(b, -1) {
			addUnique(&t.PRs, string(m))
		}
	}
	m := l.Message
	switch {
	case l.APIError && l.Error != "":
		t.Halt = &Halt{Kind: l.Error, Text: firstText(m.Content), At: l.Timestamp}
	case m.Model != "" && m.Model != "<synthetic>":
		t.Halt = nil // it answered after all
		if bytes.Contains(b, arrowMarkers[0]) || bytes.Contains(b, arrowMarkers[1]) {
			if p := progressIn(m.Content); p != "" {
				t.Progress, t.ProgressAt = p, l.Timestamp
			}
		}
	}
	if m.Usage == nil || m.Model == "" || m.Model == "<synthetic>" {
		return
	}
	if m.ID != t.PendingID {
		t.commitPending()
	}
	u := TokenUsage{Input: m.Usage.Input, Output: m.Usage.Output, CacheRead: m.Usage.CacheRead}
	if cb := m.Usage.CacheBreakup; cb != nil && cb.M5+cb.H1 > 0 {
		u.CacheWrite5m, u.CacheWrite1h = cb.M5, cb.H1
	} else {
		u.CacheWrite5m = m.Usage.CacheCreate
	}
	t.PendingID, t.PendingModel, t.PendingUse = m.ID, m.Model, u
	t.PendingFast = m.Usage.Speed == "fast"
	t.PendingAt = l.Timestamp
	t.LastModel = m.Model
}

// httpCodes are the pairs that are a status changing, not a count.
var httpCodes = map[string]bool{"200": true, "201": true, "204": true, "301": true, "302": true, "304": true,
	"400": true, "401": true, "403": true, "404": true, "409": true, "422": true, "429": true, "500": true, "502": true, "503": true}

// progressIn is the sentence of a message's text with its last count going
// from one number to another, trimmed to that pair and a little before it.
func progressIn(content jsontext.Value) string {
	var blocks []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if jsonx.Unmarshal(content, &blocks) != nil {
		return ""
	}
	best := ""
	for _, bl := range blocks {
		if bl.Type != "text" {
			continue
		}
		for _, line := range strings.Split(bl.Text, "\n") {
			for _, loc := range progressPair.FindAllStringSubmatchIndex(line, -1) {
				from, to := line[loc[2]:loc[3]], line[loc[4]:loc[5]]
				if httpCodes[from] && httpCodes[to] {
					continue
				}
				start := strings.LastIndexAny(line[:loc[0]], ".;!?|") + 1
				if loc[0]-start > 72 {
					start = loc[0] - 72
					if i := strings.IndexByte(line[start:loc[0]], ' '); i >= 0 {
						start += i + 1
					}
				}
				s := line[start:loc[1]]
				s = strings.NewReplacer("**", "", "`", "", "|", " ").Replace(s)
				s = strings.Join(strings.Fields(strings.TrimLeft(s, "-*#> :")), " ")
				if s != "" {
					best = s
				}
			}
		}
	}
	return best
}

func (t *Totals) setDir(d string, at time.Time) {
	if d != t.Dir {
		t.Dir, t.DirAt = d, at
	}
}

var toolUseMarker = []byte(`"type":"tool_use"`)

// workedIn is the folder a message's tool calls last worked in: a Bash
// command's leading `cd /abs`, or the folder of a file it wrote.
func workedIn(content jsontext.Value) (dir string, wrote bool) {
	var blocks []struct {
		Type  string `json:"type"`
		Name  string `json:"name"`
		Input struct {
			Command      string `json:"command"`
			FilePath     string `json:"file_path"`
			NotebookPath string `json:"notebook_path"`
		} `json:"input"`
	}
	if jsonx.Unmarshal(content, &blocks) != nil {
		return "", false
	}
	for _, b := range blocks {
		if b.Type != "tool_use" {
			continue
		}
		var d string
		switch b.Name {
		case "Bash":
			d = cdTarget(b.Input.Command)
		case "Edit", "Write", "MultiEdit":
			d = filepath.Dir(b.Input.FilePath)
		case "NotebookEdit":
			d = filepath.Dir(b.Input.NotebookPath)
		}
		if workFolder(d) && (b.Name != "Bash" || !wrote) {
			dir, wrote = d, b.Name != "Bash"
		}
	}
	return dir, wrote
}

// cdTarget is the folder a command starts by cd'ing into, if it does.
func cdTarget(cmd string) string {
	rest, ok := strings.CutPrefix(strings.TrimSpace(cmd), "cd ")
	if !ok {
		return ""
	}
	rest = strings.TrimSpace(rest)
	if rest != "" && (rest[0] == '"' || rest[0] == '\'') {
		if end := strings.IndexByte(rest[1:], rest[0]); end >= 0 {
			return rest[1 : end+1]
		}
		return ""
	}
	if i := strings.IndexAny(rest, " \t\n;&|)"); i >= 0 {
		rest = rest[:i]
	}
	return rest
}

// workFolder is a folder that says where a session works: absolute, and
// not scratch space or Claude Code's own (memory, plans), which any
// session writes to wherever it works.
func workFolder(d string) bool {
	if !filepath.IsAbs(d) {
		return false
	}
	for _, p := range []string{"/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/"} {
		if strings.HasPrefix(d+"/", p) {
			return false
		}
	}
	return !strings.Contains(d+"/", "/.claude/") || strings.Contains(d, "/.claude/worktrees/")
}

// firstText is the first line of a message's first text block.
func firstText(content jsontext.Value) string {
	var blocks []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if jsonx.Unmarshal(content, &blocks) != nil {
		return ""
	}
	for _, b := range blocks {
		if b.Type == "text" && b.Text != "" {
			first, _, _ := strings.Cut(strings.TrimSpace(b.Text), "\n")
			return first
		}
	}
	return ""
}

func addUnique(s *[]string, v string) {
	for _, x := range *s {
		if x == v {
			return
		}
	}
	*s = append(*s, v)
}

// SubagentTranscripts lists the transcripts of subagents a session spawned.
func SubagentTranscripts(mainPath string) []string {
	dir := strings.TrimSuffix(mainPath, ".jsonl")
	m, _ := filepath.Glob(filepath.Join(dir, "subagents", "*.jsonl"))
	return m
}

// ReadPreview reads only the last window of a transcript.
func ReadPreview(path string, window int64) Preview {
	var p Preview
	f, err := os.Open(path)
	if err != nil {
		return p
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return p
	}
	off := st.Size() - window
	if off < 0 {
		off = 0
	}
	b := make([]byte, st.Size()-off)
	if _, err := f.ReadAt(b, off); err != nil && err != io.EOF {
		return p
	}
	lines := bytes.Split(b, []byte{'\n'})
	p.Recent = recentEvents(lines, 14)
	p.First = firstUser(f, lines, off)
	for i := len(lines) - 1; i >= 0 && (p.Text == "" || p.Tool == "" || p.LastUser == "" || p.Context == 0); i-- {
		var l line
		if jsonx.Unmarshal(lines[i], &l) != nil {
			continue
		}
		if p.At.IsZero() && !l.Timestamp.IsZero() {
			p.At = l.Timestamp
		}
		var blocks []struct {
			Type  string         `json:"type"`
			Text  string         `json:"text"`
			Name  string         `json:"name"`
			Input jsontext.Value `json:"input"`
		}
		if l.Type == "user" && p.LastUser == "" {
			var s string
			if jsonx.Unmarshal(l.Message.Content, &s) == nil && s != "" && !strings.HasPrefix(s, "<") {
				p.LastUser = s
			}
			continue
		}
		if l.Type != "assistant" || jsonx.Unmarshal(l.Message.Content, &blocks) != nil {
			continue
		}
		if p.Model == "" {
			p.Model, p.Effort = l.Message.Model, l.Effort
		}
		if u := l.Message.Usage; u != nil && p.Context == 0 && l.Message.Model != "<synthetic>" {
			p.Context = u.Input + u.CacheRead + u.CacheCreate
		}
		for j := len(blocks) - 1; j >= 0; j-- {
			bl := blocks[j]
			switch {
			case bl.Type == "text" && p.Text == "" && strings.TrimSpace(bl.Text) != "":
				p.Text = strings.TrimSpace(bl.Text)
			case bl.Type == "tool_use" && p.Tool == "":
				p.Tool, p.ToolArg, p.Doing = bl.Name, toolArg(bl.Input), Doing(bl.Name, bl.Input)
			}
		}
	}
	return p
}

func toolArg(in jsontext.Value) string {
	var m map[string]any
	if jsonx.Unmarshal(in, &m) != nil {
		return ""
	}
	for _, k := range []string{"description", "command", "file_path", "pattern", "prompt", "url", "query"} {
		if s, ok := m[k].(string); ok && s != "" {
			return s
		}
	}
	return ""
}

// firstUser finds the first thing the user typed, reading the head of the
// file when the tail window does not reach back to it.
func firstUser(f *os.File, tail [][]byte, off int64) Event {
	lines := tail
	if off > 0 {
		b := make([]byte, 256<<10)
		n, _ := f.ReadAt(b, 0)
		lines = bytes.Split(b[:n], []byte{'\n'})
	}
	for _, l := range lines {
		if !bytes.Contains(l, []byte(`"type":"user"`)) {
			continue
		}
		for _, e := range recentEvents([][]byte{l}, 1) {
			if e.Role == "user" {
				return e
			}
		}
	}
	return Event{}
}

func recentEvents(lines [][]byte, n int) []Event {
	var out []Event
	for _, raw := range lines {
		var l line
		if jsonx.Unmarshal(raw, &l) != nil || (l.Type != "user" && l.Type != "assistant") {
			continue
		}
		var str string
		if l.Type == "user" && jsonx.Unmarshal(l.Message.Content, &str) == nil {
			if str != "" && !strings.HasPrefix(str, "<") {
				out = append(out, Event{Role: "user", Text: str, At: l.Timestamp})
			}
			continue
		}
		var blocks []struct {
			Type  string         `json:"type"`
			Text  string         `json:"text"`
			Name  string         `json:"name"`
			Input jsontext.Value `json:"input"`
		}
		if jsonx.Unmarshal(l.Message.Content, &blocks) != nil {
			continue
		}
		for _, bl := range blocks {
			switch {
			case bl.Type == "text" && strings.TrimSpace(bl.Text) != "":
				if l.Type == "user" {
					t := strings.TrimSpace(bl.Text)
					if strings.HasPrefix(t, "<") || strings.HasPrefix(t, "[") || strings.Contains(t, "</") {
						continue // a harness note, not something the user typed
					}
					out = append(out, Event{Role: "user", Text: t, At: l.Timestamp})
				} else {
					out = append(out, Event{Role: "assistant", Text: strings.TrimSpace(bl.Text), At: l.Timestamp})
				}
			case bl.Type == "tool_use":
				out = append(out, Event{Role: "tool", Text: bl.Name + "\x00" + toolArg(bl.Input), At: l.Timestamp})
			}
		}
	}
	if len(out) > n {
		// A copy, so the events before these aren't kept by the array.
		out = append([]Event(nil), out[len(out)-n:]...)
	}
	return out
}

type SubagentStats = agent.SubagentStats

// ReadSubagentStats counts a session's subagents from their metadata
// files: how many it ever spawned, and how many are running (written in
// the last 90s) directly and at any depth.
func ReadSubagentStats(mainPath string, now time.Time) SubagentStats {
	var st SubagentStats
	dir := filepath.Join(strings.TrimSuffix(mainPath, ".jsonl"), "subagents")
	metas, _ := filepath.Glob(filepath.Join(dir, "*.meta.json"))
	for _, meta := range metas {
		st.Spawned++
		fi, err := os.Stat(strings.TrimSuffix(meta, ".meta.json") + ".jsonl")
		if err != nil || now.Sub(fi.ModTime()) > 90*time.Second {
			continue
		}
		var m struct {
			Depth int `json:"spawnDepth"`
		}
		if b, err := os.ReadFile(meta); err == nil {
			_ = jsonx.Unmarshal(b, &m)
		}
		if m.Depth <= 1 {
			st.Direct++
		} else {
			st.Nested++
		}
	}
	return st
}
