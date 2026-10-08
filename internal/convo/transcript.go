package convo

import (
	"bytes"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Tail follows a Claude Code transcript file, so a session rush doesn't
// run itself draws exactly like one it does: the transcript holds the same
// messages the stream carries. Each Read takes in only what's new.
type Tail struct {
	Path string
	Sess *Session

	off       atomic.Int64 // Size asks it while a Fetch may be moving it
	partial   []byte
	sidechain bool      // a subagent's own transcript: its lines are the story
	before    time.Time // History: only lines from before this
	past      bool      // History: read past before, so there's nothing more to take
	cut       bool      // NewTailFrom: the first line read is the end of one, dropped
	// Stop, once set, has Read give up where it is.
	Stop *atomic.Bool
	// Each, when set, is given each whole line Read takes, as it's read.
	Each func(line []byte)
}

var readBufs = sync.Pool{New: func() any { b := make([]byte, 64<<10); return &b }}

func NewTail(path string) *Tail { return &Tail{Path: path, Sess: New()} }

// NewTailFrom is a Tail that starts at the first whole line of the file's
// last most bytes, so a long conversation's end is read at once; its
// Session is then Partial.
func NewTailFrom(path string, most int64) *Tail {
	t := NewTail(path)
	if st, err := os.Stat(path); err == nil && st.Size() > most {
		// A byte early: the line cut short there runs to the first newline,
		// which is that byte when the cut fell between two lines.
		t.off.Store(st.Size() - most - 1)
		t.cut, t.Sess.Partial = true, true
	}
	return t
}

// Size is how far into the file Read has got.
func (t *Tail) Size() int64 { return t.off.Load() }

// History is the conversation a transcript holds from before a moment: what
// a rush session that resumed one had already said before its host
// started (the host replays the rest). Its last turn is closed.
func History(path string, before time.Time) *Session { return HistoryFrom(path, before, 0, nil) }

// HistoryFrom is History read from the first whole line at or after byte
// from, its Session Partial when that's past the start; once stop is set
// it gives up, and says nothing.
func HistoryFrom(path string, before time.Time, from int64, stop *atomic.Bool) *Session {
	t := NewTail(path)
	t.before, t.Stop = before, stop
	if from > 0 {
		t.off.Store(from - 1) // as NewTailFrom
		t.cut, t.Sess.Partial = true, true
	}
	if _, err := t.Read(); err != nil || stop != nil && stop.Load() {
		return New()
	}
	if live := t.Sess.Live(); live != nil {
		t.Sess.Apply(event.TurnEnd{Reason: "done"}, live.Start)
	}
	return t.Sess
}

// HistoryStart is where to read a transcript from for the last most bytes
// written before at: at's line is found by its timestamps, a handful of
// small reads, so the rest of the file is never read.
func HistoryStart(path string, at time.Time, most int64) int64 {
	f, err := os.Open(path)
	if err != nil {
		return 0
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return 0
	}
	lo, hi := int64(0), st.Size()
	buf := make([]byte, 64<<10)
	for hi-lo > max(most/8, 64<<10) {
		mid := lo + (hi-lo)/2
		if t, ok := stampAfter(f, mid, buf); ok && t.Before(at) {
			lo = mid
		} else {
			hi = mid // unknown reads as later: the tail starts earlier
		}
	}
	return max(0, hi-most)
}

var stampMark = []byte(`"timestamp":"`)

// stampAfter is the time of the first whole line after byte off that
// says one, looked for over up to a megabyte: a tool's output can be long.
func stampAfter(f *os.File, off int64, buf []byte) (time.Time, bool) {
	for range 16 {
		n, _ := f.ReadAt(buf, off)
		lines := bytes.Split(buf[:n], []byte{'\n'})
		// The first may have begun before off, and the last may run on.
		for i := 1; i < len(lines)-1; i++ {
			if t, ok := lineStamp(lines[i]); ok {
				return t, true
			}
		}
		if n < len(buf) {
			break
		}
		off += int64(n)
	}
	return time.Time{}, false
}

// lineStamp is a transcript line's own time: its last stamp, since the
// message before it can hold tool inputs that have one.
func lineStamp(b []byte) (time.Time, bool) {
	k := bytes.LastIndex(b, stampMark)
	if k < 0 {
		return time.Time{}, false
	}
	b = b[k+len(stampMark):]
	e := bytes.IndexByte(b, '"')
	if e < 0 {
		return time.Time{}, false
	}
	t, err := time.Parse(time.RFC3339Nano, string(b[:e]))
	return t, err == nil
}

type tline struct {
	Type          string         `json:"type"`
	Subtype       string         `json:"subtype"`
	IsMeta        bool           `json:"isMeta"`
	IsSidechain   bool           `json:"isSidechain"`
	Timestamp     time.Time      `json:"timestamp"`
	Message       jsontext.Value `json:"message"`
	ToolUseResult jsontext.Value `json:"toolUseResult"`
	Cwd           string         `json:"cwd"`
	Effort        string         `json:"effort"`
	Content       jsontext.Value `json:"content"`
	Level         string         `json:"level"`
	Attachment    struct {
		Type      string         `json:"type"`
		HookEvent string         `json:"hookEvent"`
		Content   jsontext.Value `json:"content"`
	} `json:"attachment"`
	Compact struct {
		Trigger    string `json:"trigger"`
		PreTokens  int    `json:"preTokens"`
		PostTokens int    `json:"postTokens"`
	} `json:"compactMetadata"`
}

// Read applies whatever has been appended since the last call and reports
// whether anything changed.
func (t *Tail) Read() (bool, error) {
	// Nothing new costs one stat.
	st, err := os.Stat(t.Path)
	if err != nil {
		return false, err
	}
	if st.Size() < t.off.Load() { // rewritten from scratch: start over
		// Reset in place: whoever holds the Session keeps following it.
		t.off.Store(0)
		t.partial = nil
		*t.Sess = *New()
	}
	if st.Size() == t.off.Load() {
		return false, nil
	}
	f, err := os.Open(t.Path)
	if err != nil {
		return false, err
	}
	defer f.Close()
	if _, err := f.Seek(t.off.Load(), io.SeekStart); err != nil {
		return false, err
	}
	// One read buffer shared by every tail: a session can follow hundreds
	// of subagent runs, and each holding its own would cost megabytes.
	bp := readBufs.Get().(*[]byte)
	defer readBufs.Put(bp)
	buf := *bp
	changed := false
	for {
		n, err := f.Read(buf)
		t.off.Add(int64(n))
		chunk := buf[:n]
		for len(chunk) > 0 {
			i := bytes.IndexByte(chunk, '\n')
			if i < 0 {
				// An unfinished last line waits for the rest of it.
				t.partial = append(t.partial, chunk...)
				break
			}
			line := chunk[:i+1]
			chunk = chunk[i+1:]
			if len(t.partial) > 0 {
				line = append(t.partial, line...)
				// Not kept for the next line: a tail mostly sits idle, and
				// hundreds of them each holding a buffer add up.
				t.partial = nil
			}
			if t.cut {
				t.cut = false
				continue
			}
			if t.Each != nil {
				t.Each(line)
			}
			if t.apply(bytes.TrimSpace(line)) {
				changed = true
			}
			if t.past {
				return changed, nil
			}
		}
		if err != nil || n == 0 || t.Stop != nil && t.Stop.Load() {
			break
		}
	}
	return changed, nil
}

// Fresh is what a transcript gained since its Tail last fetched: whole
// lines, not yet applied.
type Fresh struct {
	reset  bool // the file was rewritten: start the conversation over
	lines  []byte
	parsed []parsedLine // lines, parsed off the UI's goroutine; nil for a light session
}

// Fetch reads what the file has gained, and decodes it, without applying
// it. It touches only where the tail is in the file, never Sess, so it can
// run off the UI's goroutine while Sess is drawn; Take then applies what
// it read.
// Only one Fetch or Read of a tail may run at a time.
func (t *Tail) Fetch() (Fresh, error) {
	var f Fresh
	st, err := os.Stat(t.Path)
	if err != nil {
		return f, err
	}
	off := t.off.Load()
	if st.Size() < off {
		f.reset, off, t.partial = true, 0, nil
		t.off.Store(0)
	}
	if st.Size() == off {
		return f, nil
	}
	fh, err := os.Open(t.Path)
	if err != nil {
		return f, err
	}
	defer fh.Close()
	buf := make([]byte, len(t.partial), len(t.partial)+int(st.Size()-off))
	copy(buf, t.partial)
	n, err := fh.ReadAt(buf[len(buf):cap(buf)], off)
	if err != nil && err != io.EOF {
		return f, err
	}
	t.off.Store(off + int64(n))
	buf = buf[:len(buf)+n]
	// An unfinished last line waits for the rest of it.
	i := bytes.LastIndexByte(buf, '\n')
	t.partial = nil
	if i+1 < len(buf) {
		t.partial = bytes.Clone(buf[i+1:])
	}
	f.lines = buf[:i+1]
	if !t.Sess.light { // set as the tail is made, never after
		for rest := f.lines; len(rest) > 0; {
			line := rest
			if i := bytes.IndexByte(rest, '\n'); i >= 0 {
				line, rest = rest[:i], rest[i+1:]
			} else {
				rest = nil
			}
			if p, ok := t.parse(bytes.TrimSpace(line)); ok {
				f.parsed = append(f.parsed, p)
			}
		}
	}
	return f, nil
}

// Take applies what Fetch read, and reports whether anything changed.
func (t *Tail) Take(f Fresh) bool {
	changed := f.reset
	if f.reset {
		*t.Sess = *New() // in place: whoever holds the Session keeps following it
	}
	if f.parsed != nil || !t.Sess.light {
		for i := range f.parsed {
			if t.take(&f.parsed[i]) {
				changed = true
			}
		}
		return changed
	}
	for rest := f.lines; len(rest) > 0; {
		line := rest
		if i := bytes.IndexByte(rest, '\n'); i >= 0 {
			line, rest = rest[:i], rest[i+1:]
		} else {
			rest = nil
		}
		if t.apply(bytes.TrimSpace(line)) {
			changed = true
		}
	}
	return changed
}

func (t *Tail) apply(b []byte) bool {
	if len(b) == 0 {
		return false
	}
	if t.Sess.light {
		return t.applyLight(b)
	}
	p, ok := t.parse(b)
	return ok && t.take(&p)
}

// parsedLine is a transcript line as parse decoded it: all the JSON a
// line holds, read without touching the Session, so Fetch can do it off
// the UI's goroutine.
type parsedLine struct {
	l tline
	// A user line: what you typed, when it's that.
	text     string
	images   []string
	pictures []*event.ImageData
	prompt   bool
	hooks    []Hook // what a start hook added
	content  jsontext.Value
	notice   string // a system line's text
	ev       any    // a message as the stream would carry it
	evFailed bool
}

// parse decodes a line for take; ok is false when it's not one to take.
func (t *Tail) parse(b []byte) (p parsedLine, ok bool) {
	if len(b) == 0 {
		return p, false
	}
	l := &p.l
	if jsonx.Unmarshal(b, l) != nil || l.IsSidechain != t.sidechain || l.IsMeta {
		return p, false
	}
	switch l.Type {
	case "system":
		if l.Subtype == "informational" || l.Subtype == "local_command" {
			_ = jsonx.Unmarshal(l.Content, &p.notice)
		}
	case "user":
		var m struct {
			Content jsontext.Value `json:"content"`
		}
		_ = jsonx.Unmarshal(l.Message, &m)
		p.content = m.Content
		if p.text, p.images, p.prompt = prompt(m.Content); p.prompt {
			if len(p.images) > 0 {
				p.pictures = promptPictures(m.Content)
			}
			break
		}
		fallthrough
	case "assistant":
		p.evFailed = true
		if n := native(); n != nil {
			ev, err := n.Message(l.Type, l.Message, l.ToolUseResult)
			p.ev, p.evFailed = ev, err != nil
		}
	case "attachment":
		p.text, p.prompt = told(l.Attachment.Type, l.Attachment.Content)
		if k := l.Attachment.Type; k == "hook_success" || k == "hook_additional_context" {
			p.hooks = hookNotes(k, l.Attachment.HookEvent, l.Attachment.Content)
		}
	}
	// What's been decoded isn't kept twice.
	l.Message, l.ToolUseResult, l.Content, l.Attachment.Content = nil, nil, nil, nil
	return p, true
}

// take applies a parsed line to the Session.
func (t *Tail) take(p *parsedLine) bool {
	t.Sess.applied++ // what follows may change it without Apply
	l := &p.l
	if !t.before.IsZero() && !l.Timestamp.IsZero() && !l.Timestamp.Before(t.before) {
		// A transcript is written in order: once well past before (a
		// minute, for lines written a little out of it), the rest is
		// after it too, and needn't be read. A resumed session's host can
		// have run for days, and all it wrote since was read to be dropped.
		t.past = l.Timestamp.After(t.before.Add(time.Minute))
		return false
	}
	s := t.Sess
	if l.Cwd != "" && s.Cwd == "" {
		s.Cwd = l.Cwd
	}
	at := l.Timestamp
	switch l.Type {
	case "system":
		if l.Subtype == "turn_duration" {
			s.Apply(event.TurnEnd{Reason: "done"}, at)
			return true
		}
		if l.Subtype == "informational" || l.Subtype == "local_command" {
			// Claude Code telling you something (an unknown command, a
			// warning): shown where it happened.
			if text := p.notice; strings.TrimSpace(text) != "" {
				s.notice(stripTags(text), l.Level, at)
				return true
			}
			return false
		}
		if l.Subtype == "compact_boundary" {
			c := l.Compact
			s.Apply(event.Compacted{Trigger: c.Trigger, Before: c.PreTokens, After: c.PostTokens}, at)
			return true
		}
		return false
	case "user":
		if text, images := p.text, p.images; p.prompt {
			// A shell command you ran with ! is its own small turn, and
			// its output lands on it rather than starting another.
			if out, ok := shellOutput(text); ok {
				if live := s.Live(); live != nil && strings.HasPrefix(live.Prompt, "! ") {
					s.shellResult(live, out, at)
				}
				return true
			}
			// The summary a compaction leaves goes with its divider.
			if strings.HasPrefix(strings.TrimSpace(text), "This session is being continued from a previous conversation") {
				if s.compactSummary(text) {
					return true
				}
			}
			// You stopped it: that ends the turn, it isn't a new one.
			if strings.HasPrefix(strings.TrimSpace(text), "[Request interrupted by user") {
				s.interrupted(at)
				return true
			}
			// A new prompt closes a turn the transcript never marked done,
			// and the one a Partial session began part way through.
			if live := s.Live(); live != nil && (live.Prompt != "" || s.Partial && live.N == 1) {
				s.Apply(event.TurnEnd{Reason: "done"}, at)
			}
			from, text2, injected := Injected(text)
			if injected {
				text = text2
				s.noteTask(p.content)
			}
			e := s.sent(host.Sent{Text: text, Images: images}, at, false)
			if e.item != nil {
				e.item.Pictures = p.pictures
			} else {
				e.turn.Pictures = p.pictures
			}
			if injected {
				s.Turns[len(s.Turns)-1].From = from
			}
			if cmd, ok := strings.CutPrefix(text, "! "); ok {
				s.shellStart(cmd, at)
			}
			if n := len(s.Turns); n > 0 && l.Effort != "" {
				s.Turns[n-1].Effort = l.Effort
			}
			return true
		}
		fallthrough
	case "assistant":
		ev := p.ev
		if p.evFailed {
			return false
		}
		if l.Type == "assistant" {
			if live := s.Live(); live != nil && l.Effort != "" && live.Effort == "" {
				live.Effort = l.Effort
			}
		}
		s.Apply(ev, at)
		return true
	case "attachment":
		// A message rush's inbox handed over mid-turn: yours, as you sent it.
		if p.prompt {
			s.Apply(host.Sent{Text: p.text}, at)
		}
		if len(p.hooks) > 0 {
			s.addHooks(p.hooks)
			return true
		}
		return p.prompt
	}
	return false
}

// told is the message rush's inbox hook handed the agent, from the
// attachment Claude Code keeps of what the hook added.
func told(kind string, content jsontext.Value) (string, bool) {
	var texts []string
	if kind != "hook_additional_context" || jsonx.Unmarshal(content, &texts) != nil {
		return "", false
	}
	for _, t := range texts {
		if msg, ok := strings.CutPrefix(t, host.TellNote); ok {
			return msg, true
		}
	}
	return "", false
}

// prompt reads a user line as something you typed: plain text or text and
// image blocks, not a tool result. Command wrappers are unwrapped.
func prompt(raw jsontext.Value) (string, []string, bool) {
	var s string
	if jsonx.Unmarshal(raw, &s) == nil {
		return cleanPrompt(s), nil, strings.TrimSpace(s) != ""
	}
	var blocks []struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	if jsonx.Unmarshal(raw, &blocks) != nil || len(blocks) == 0 {
		return "", nil, false
	}
	var texts, images []string
	for _, b := range blocks {
		switch b.Type {
		case "text":
			texts = append(texts, b.Text)
		case "image":
			images = append(images, "image")
		default:
			return "", nil, false // a tool result
		}
	}
	return cleanPrompt(joinTexts(texts)), images, true
}

var endsWithImage = regexp.MustCompile(`\[Image #\d+\]$`)

// joinTexts puts a message's text blocks back together. Text split around an
// image at its [Image #N] marker was one text, so its pieces join as they
// were; other blocks are separate paragraphs.
func joinTexts(texts []string) string {
	var b strings.Builder
	for i, t := range texts {
		if i > 0 && !endsWithImage.MatchString(texts[i-1]) {
			b.WriteByte('\n')
		}
		b.WriteString(t)
	}
	return b.String()
}

// cleanPrompt drops the markup Claude Code wraps around slash commands and
// system notes, keeping what you actually said.
func cleanPrompt(s string) string {
	if cmd := between(s, "<bash-input>", "</bash-input>"); cmd != "" {
		return "! " + cmd
	}
	if strings.Contains(s, "<bash-stdout>") || strings.Contains(s, "<bash-stderr>") {
		return s // kept whole for shellOutput
	}
	if name := between(s, "<command-name>", "</command-name>"); name != "" {
		args := between(s, "<command-args>", "</command-args>")
		return strings.TrimSpace(name + " " + args)
	}
	for _, tag := range []string{"system-reminder", "local-command-stdout", "local-command-caveat"} {
		for {
			i := strings.Index(s, "<"+tag+">")
			j := strings.Index(s, "</"+tag+">")
			if i < 0 || j < i {
				break
			}
			s = s[:i] + s[j+len(tag)+3:]
		}
	}
	return strings.TrimSpace(s)
}

func between(s, a, b string) string {
	i := strings.Index(s, a)
	if i < 0 {
		return ""
	}
	rest := s[i+len(a):]
	j := strings.Index(rest, b)
	if j < 0 {
		return ""
	}
	return strings.TrimSpace(rest[:j])
}

// shellOutput reads the line Claude Code writes after a ! command.
func shellOutput(s string) (shellOut, bool) {
	if !strings.Contains(s, "<bash-stdout>") && !strings.Contains(s, "<bash-stderr>") {
		return shellOut{}, false
	}
	return shellOut{stdout: between(s, "<bash-stdout>", "</bash-stdout>"), stderr: between(s, "<bash-stderr>", "</bash-stderr>")}, true
}

type shellOut struct{ stdout, stderr string }

func (s *Session) shellStart(cmd string, at time.Time) {
	t := s.Live()
	if t == nil {
		return
	}
	id := fmt.Sprintf("you-%d", t.N)
	in, _ := jsonx.Marshal(map[string]string{"command": cmd, "description": "you ran"})
	st := &Step{ID: id, Tool: "Bash", Kind: tool.Shell, Input: in, Start: at, Exit: -1, turn: t}
	st.read()
	s.byID[id] = st
	t.steps[id] = st
	s.stepVer++
	t.Items = append(t.Items, &Item{Kind: KStep, Step: st})
	t.touch()
}

func (s *Session) shellResult(t *Turn, out shellOut, at time.Time) {
	st := s.byID[fmt.Sprintf("you-%d", t.N)]
	if st == nil {
		return
	}
	stdout := out.stdout
	if stdout == "(Bash completed with no output)" {
		stdout = ""
	}
	st.Output, st.End, st.Status = strings.TrimSpace(stdout+"\n"+out.stderr), at, OK
	st.Result, _ = jsonx.Marshal(map[string]string{"stdout": stdout, "stderr": out.stderr})
	if strings.TrimSpace(out.stderr) != "" {
		st.Status = Failed
	}
	st.readOutput(st.Status == Failed)
	s.Apply(event.TurnEnd{Reason: "done"}, at)
}

// Injected recognises text Claude Code puts in a user message that you
// didn't type, and says who it's from and what it says in a line.
func Injected(s string) (from, text string, ok bool) {
	t := strings.TrimSpace(s)
	switch {
	case strings.HasPrefix(t, "<task-notification>"):
		status := taskStatus(between(t, "<status>", "</status>"))
		sum := between(t, "<summary>", "</summary>")
		from = "background task"
		if status != "" {
			from += " · " + status
		}
		return from, firstLine(firstNonEmpty(sum, "finished")), true
	case strings.HasPrefix(t, "<cross-session-message"):
		name := attr(t, "from-name")
		body := t[strings.Index(t, ">")+1:]
		body = strings.TrimSuffix(strings.TrimSpace(body), "</cross-session-message>")
		return "message from " + firstNonEmpty(name, "another session"), firstLine(body), true
	case strings.HasPrefix(t, "<agent-message"):
		body := t[strings.Index(t, ">")+1:]
		body = strings.TrimSuffix(strings.TrimSpace(body), "</agent-message>")
		return "a subagent reported back", firstLine(stripTags(body)), true
	case strings.HasPrefix(t, "<") && strings.Contains(t, "</"):
		// Some other wrapper: drop the tags, keep the words.
		return "Claude Code", firstLine(stripTags(t)), true
	}
	return "", s, false
}

// taskStatus is a background task's status as rush says it: one you
// stopped is stopped, though Claude Code calls it killed.
func taskStatus(s string) string {
	if s == "killed" {
		return "stopped"
	}
	return s
}

func attr(s, name string) string {
	i := strings.Index(s, name+`="`)
	if i < 0 {
		return ""
	}
	rest := s[i+len(name)+2:]
	if j := strings.IndexByte(rest, '"'); j >= 0 {
		return rest[:j]
	}
	return ""
}

var tagRe = regexp.MustCompile(`<[^>]{1,80}>`)

func stripTags(s string) string { return strings.TrimSpace(tagRe.ReplaceAllString(s, " ")) }

// noteTask records a background task's reported status, from the
// notification Claude Code injects when one finishes.
func (s *Session) noteTask(raw jsontext.Value) {
	var t string
	if jsonx.Unmarshal(raw, &t) != nil {
		var blocks []struct {
			Text string `json:"text"`
		}
		_ = jsonx.Unmarshal(raw, &blocks)
		for _, b := range blocks {
			t += b.Text
		}
	}
	if id := between(t, "<task-id>", "</task-id>"); id != "" {
		s.TaskStatus[id] = firstNonEmpty(taskStatus(between(t, "<status>", "</status>")), "completed")
	}
}

// compactSummary puts the summary a compaction leaves on its divider. It
// reports whether there was a divider to put it on.
func (s *Session) compactSummary(text string) bool {
	for i := len(s.Turns) - 1; i >= 0 && i >= len(s.Turns)-2; i-- {
		items := s.Turns[i].Items
		for j := len(items) - 1; j >= 0; j-- {
			if items[j].Kind == KCompact && items[j].Text == "" {
				items[j].Text = text
				s.Turns[i].touch()
				return true
			}
		}
	}
	return false
}

// lightLine is the little of a transcript line a subagent's row needs. Tool
// inputs and results aren't declared, so decoding skips them unread.
type lightLine struct {
	Type        string    `json:"type"`
	Subtype     string    `json:"subtype"`
	IsSidechain bool      `json:"isSidechain"`
	IsMeta      bool      `json:"isMeta"`
	Timestamp   time.Time `json:"timestamp"`
	Message     struct {
		ID      string       `json:"id"`
		Role    string       `json:"role"`
		Model   string       `json:"model"`
		Usage   *lightUsage  `json:"usage"`
		Content lightContent `json:"content"`
	} `json:"message"`
}

// lightUsage is what a request used, as a transcript line says.
type lightUsage struct {
	Input      int64 `json:"input_tokens"`
	Output     int64 `json:"output_tokens"`
	CacheRead  int64 `json:"cache_read_input_tokens"`
	CacheWrite int64 `json:"cache_creation_input_tokens"`
	Breakup    *struct {
		M5 int64 `json:"ephemeral_5m_input_tokens"`
		H1 int64 `json:"ephemeral_1h_input_tokens"`
	} `json:"cache_creation,omitempty"`
}

// lightContent is a message's content: a prompt as plain text, or blocks.
type lightContent struct {
	text   string
	blocks []lightBlock
}

type lightBlock struct {
	Type      string         `json:"type"`
	Text      string         `json:"text"`
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Input     jsontext.Value `json:"input"` // read for the call in words, then dropped
	ToolUseID string         `json:"tool_use_id"`
	IsError   bool           `json:"is_error"`
}

func (c *lightContent) UnmarshalJSON(b []byte) error {
	if len(b) > 0 && b[0] == '"' {
		return jsonx.Unmarshal(b, &c.text)
	}
	return jsonx.Unmarshal(b, &c.blocks)
}

// applyLight takes in a line for a light session (SubagentStats).
func (t *Tail) applyLight(b []byte) bool {
	var l lightLine
	if jsonx.Unmarshal(b, &l) != nil || l.IsSidechain != t.sidechain || l.IsMeta {
		return false
	}
	s, at := t.Sess, l.Timestamp
	switch l.Type {
	case "system":
		if l.Subtype == "turn_duration" {
			s.Apply(event.TurnEnd{Reason: "done"}, at)
			return true
		}
		return false
	case "user", "assistant":
	default:
		return false
	}
	c := l.Message.Content
	if l.Type == "user" {
		text, prompt := c.text, c.text != ""
		if !prompt && len(c.blocks) > 0 {
			prompt = true
			for _, bl := range c.blocks {
				if bl.Type != "text" && bl.Type != "image" {
					prompt = false
				}
				if bl.Type == "text" && text == "" {
					text = bl.Text
				}
			}
		}
		if prompt {
			if live := s.Live(); live != nil && live.Prompt != "" {
				s.Apply(event.TurnEnd{Reason: "done"}, at)
			}
			s.Apply(host.Sent{Text: strings.Clone(firstLine(cleanPrompt(text)))}, at)
			return true
		}
	}
	m := event.Message{Role: l.Type, ID: l.Message.ID, Model: l.Message.Model, Injected: l.Type == "user"}
	if u := l.Message.Usage; u != nil {
		// Cache writes not split by lifetime count as the hour Claude
		// Code asks for its main agent; subagents write the 5 minutes.
		m.Tokens = &usage.TokenUsage{Input: u.Input, Output: u.Output, CacheRead: u.CacheRead, CacheWrite1h: u.CacheWrite}
		if b := u.Breakup; b != nil && b.M5+b.H1 > 0 {
			m.Tokens.CacheWrite5m, m.Tokens.CacheWrite1h = b.M5, b.H1
		}
	}
	if s.calls == nil {
		s.calls = map[string]tool.Call{}
	}
	for _, bl := range c.blocks {
		switch bl.Type {
		case "text":
			if l.Type == "assistant" && strings.TrimSpace(bl.Text) != "" {
				m.Parts = append(m.Parts, event.Part{Kind: event.Text, Text: strings.Clone(firstPlain(bl.Text))})
			}
		case "tool_use":
			call := nativeCall(bl.ID, bl.Name, bl.Input)
			// Only what its result is read by: a whole file written or a
			// prompt, kept for every call of every run, came to megabytes.
			s.calls[bl.ID] = tool.Call{ID: call.ID, Name: call.Name, Kind: call.Kind}
			m.Parts = append(m.Parts, event.Part{Kind: event.ToolCall, Call: &call})
		case "tool_result":
			call, ok := s.calls[bl.ToolUseID]
			if !ok {
				call = tool.Call{ID: bl.ToolUseID}
			}
			delete(s.calls, bl.ToolUseID)
			o := nativeOutput(call, "", bl.IsError, nil)
			m.Parts = append(m.Parts, event.Part{Kind: event.ToolResult, Output: &o})
		}
	}
	if len(m.Parts) == 0 && m.Tokens == nil {
		return false
	}
	s.Apply(m, at)
	return true
}

// promptPictures decodes attachments once, while a history line is read off the UI.
func promptPictures(raw jsontext.Value) []*event.ImageData {
	var blocks []struct {
		Type   string `json:"type"`
		Source struct {
			Data      []byte `json:"data"`
			MediaType string `json:"media_type"`
			URL       string `json:"url"`
		} `json:"source"`
	}
	if jsonx.Unmarshal(raw, &blocks) != nil {
		return nil
	}
	var out []*event.ImageData
	for _, b := range blocks {
		if b.Type == "image" {
			out = append(out, &event.ImageData{Data: b.Source.Data, MediaType: b.Source.MediaType, Path: b.Source.URL})
		}
	}
	return out
}
