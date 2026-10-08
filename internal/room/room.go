// Package room is a group chat of fresh agents on one topic: they take
// turns, see everything said, argue, and end on a verdict for you. A
// driver process (rush room run) runs the turns through ordinary rush
// sessions; the room is one append-only log any client tails and posts
// to. See docs/room.md.
package room

import (
	"bufio"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// User is who you are in a room's log.
const User = "user"

// Entry kinds.
const (
	Turn    = "turn"    // From starts speaking
	Say     = "say"     // a message: an agent's turn, or yours
	Note    = "note"    // an agent's words on the way, between tool calls
	Tool    = "tool"    // a tool call, by ID; a later one with the same ID replaces it
	Done    = "done"    // tool call ID came back (Failed when it failed)
	Verdict = "verdict" // the room's answer: each member's final position, a line each; Detail says why it ended
	Control = "control" // yours: Text is stop, verdict, pause or resume
	Paused  = "paused"  // From's turn was cut short by a pause
	Uses    = "model"   // From runs model Text: its default, as its session says
	Joined  = "joined"  // From's session started: ID is its rush session id
	Info    = "info"    // the room's own notices
	End     = "end"     // the room is over; Text says why
)

// Entry is one line of a room's log.
type Entry struct {
	At     time.Time  `json:"at"`
	From   string     `json:"from"`
	Kind   string     `json:"kind"`
	Text   string     `json:"text,omitempty"`
	Detail string     `json:"detail,omitempty"` // a tool call's command or path
	To     string     `json:"to,omitempty"`     // a message of yours @someone
	ID     string     `json:"id,omitempty"`     // a tool call's
	Failed bool       `json:"failed,omitzero"`
	Ready  string     `json:"ready,omitempty"` // a turn's READY: yes or no
	Round  int        `json:"round,omitzero"`
	Call   *tool.Call `json:"call,omitempty"` // a tool call as made, for the pane to draw as a step
}

// Member is one agent on the panel.
type Member struct {
	Name   string      `json:"name"`
	Config host.Config `json:"config"`
	// Context is its model's context window in tokens, when known: a
	// small one keeps prompts short.
	Context int `json:"context,omitzero"`
}

// Label is the member's agent in words: "Opus 5.5 · Claude Code".
func (m Member) Label() string {
	k := agent.Kind(m.Config.Kind)
	if m.Config.Model == "" {
		return agent.HarnessLabel(k)
	}
	return agent.ModelName(k, m.Config.Model) + " · " + agent.HarnessLabel(k)
}

// Room is how a room was opened.
type Room struct {
	ID      string    `json:"id"`
	Topic   string    `json:"topic"`
	Cwd     string    `json:"cwd"`
	Rounds  int       `json:"rounds"` // the most rounds before a verdict is forced
	Members []Member  `json:"members"`
	Created time.Time `json:"created"`
	// From is the chat the room was opened from, by its agent key: the
	// room shows in that chat and its verdict goes back there.
	From string `json:"from,omitempty"`
	// Brief is what of that chat every member reads with the topic.
	Brief string `json:"brief,omitempty"`
}

// DefaultRounds caps a room that never converges.
const DefaultRounds = 4

// Root is where rooms are kept.
func Root() string { return filepath.Join(state.Dir(), "rooms") }

func dir(id string) string     { return filepath.Join(Root(), id) }
func logPath(id string) string { return filepath.Join(dir(id), "log.jsonl") }
func pidPath(id string) string { return filepath.Join(dir(id), "driver.pid") }

// Name gives each member a short name to be called by: the model's first
// word ("Opus"), else the harness's ("Codex"), numbered when taken.
func Name(kind, model string, taken []string) string {
	k := agent.Kind(kind)
	base := agent.HarnessLabel(k)
	if n := agent.ModelName(k, model); n != model {
		base = n // the adapter's name for it: "Opus 5.5"
	} else if f := strings.FieldsFunc(model, func(r rune) bool { return strings.ContainsRune("-_./:", r) }); len(f) > 0 && len(f[len(f)-1]) >= 3 &&
		!strings.ContainsFunc(f[len(f)-1], func(r rune) bool { return r < 'a' || r > 'z' }) {
		base = strings.ToUpper(f[len(f)-1][:1]) + f[len(f)-1][1:] // "gpt-6-luna" is Luna; "gpt-5.5" stays the harness's
	}
	base = strings.Fields(base + " Agent")[0]
	base = strings.Trim(base, "@:,.")
	name := base
	for n := 2; slices.ContainsFunc(taken, func(t string) bool { return strings.EqualFold(t, name) }); n++ {
		name = base + strconv.Itoa(n)
	}
	return name
}

// Create keeps r as a new room, its id filled in.
func Create(r Room) (Room, error) {
	if strings.TrimSpace(r.Topic) == "" {
		return r, errors.New("a room needs a topic")
	}
	if len(r.Members) < 2 {
		return r, errors.New("a room needs at least two agents")
	}
	if r.Rounds <= 0 {
		r.Rounds = DefaultRounds
	}
	if r.ID == "" {
		_, r.ID = host.NewSessionID()
	}
	r.Created = time.Now()
	if err := os.MkdirAll(dir(r.ID), 0o700); err != nil {
		return r, err
	}
	b, err := jsonx.MarshalIndent(r)
	if err != nil {
		return r, err
	}
	return r, os.WriteFile(filepath.Join(dir(r.ID), "room.json"), b, 0o600)
}

// Load reads room id.
func Load(id string) (Room, error) {
	var r Room
	if id == "" || strings.ContainsAny(id, "/\\") || id == "." || id == ".." {
		return r, fmt.Errorf("no room %q", id)
	}
	b, err := os.ReadFile(filepath.Join(dir(id), "room.json"))
	if err != nil {
		return r, fmt.Errorf("no room %q", id)
	}
	return r, jsonx.Unmarshal(b, &r)
}

// List is every room, newest first.
func List() []Room {
	ents, _ := os.ReadDir(Root())
	var out []Room
	for _, e := range ents {
		if r, err := Load(e.Name()); err == nil {
			out = append(out, r)
		}
	}
	slices.SortFunc(out, func(a, b Room) int { return b.Created.Compare(a.Created) })
	return out
}

// Start runs the room's driver as a process of its own, so closing rush
// leaves the room going.
func Start(id string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	log, err := os.OpenFile(filepath.Join(dir(id), "driver.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer log.Close()
	cmd := exec.Command(exe, "room", "run", id)
	cmd.Stdout, cmd.Stderr = log, log
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return err
	}
	go func() { _ = cmd.Wait() }() // reaped, or it lingers as a zombie that looks alive
	return nil
}

// Running is whether room id's driver is still going.
func Running(id string) bool {
	b, err := os.ReadFile(pidPath(id))
	if err != nil {
		return false
	}
	pid, _ := strconv.Atoi(strings.TrimSpace(string(b)))
	return pid > 0 && host.Alive(pid)
}

// Post adds e to room id's log.
// ponytail: driver and clients append to one file; each line is a single
// O_APPEND write, which keeps lines whole on a local disk. A socket if
// rooms ever span machines.
func Post(id string, e Entry) error {
	if e.At.IsZero() {
		e.At = time.Now()
	}
	b, err := jsonx.Marshal(e)
	if err != nil {
		return err
	}
	f, err := os.OpenFile(logPath(id), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	_, err = f.Write(append(b, '\n'))
	return err
}

// Read is room id's log from byte off: the whole lines there, and the
// offset after them.
func Read(id string, off int64) ([]Entry, int64, error) {
	f, err := os.Open(logPath(id))
	if errors.Is(err, os.ErrNotExist) {
		return nil, off, nil
	}
	if err != nil {
		return nil, off, err
	}
	defer f.Close()
	if _, err := f.Seek(off, 0); err != nil {
		return nil, off, err
	}
	var out []Entry
	r := bufio.NewReaderSize(f, 64<<10)
	for {
		line, err := r.ReadBytes('\n')
		if err != nil {
			break // a partial last line waits for its newline
		}
		off += int64(len(line))
		var e Entry
		if jsonx.Unmarshal(line, &e) == nil {
			out = append(out, e)
		}
	}
	return out, off, nil
}

// Parse is a message of yours as you'd type it: "@codex why?" is to Codex;
// /stop, /verdict, /pause and /resume are controls.
func Parse(text string, members []Member) Entry {
	text = strings.TrimSpace(text)
	switch strings.ToLower(text) {
	case "/stop", "/verdict", "/pause", "/resume":
		return Entry{From: User, Kind: Control, Text: strings.ToLower(text[1:])}
	}
	e := Entry{From: User, Kind: Say, Text: text}
	if at, ok := strings.CutPrefix(text, "@"); ok {
		word, _, _ := strings.Cut(at, " ")
		word = strings.TrimRight(word, ",:")
		for _, m := range members {
			if strings.EqualFold(m.Name, word) {
				e.To = m.Name
			}
		}
	}
	return e
}

var readyLine = regexp.MustCompile("(?i)[*_`]*\\bREADY[*_`\\s]*:[*_`\\s]*(yes|no)\\b[*_`.]*")

// Ready takes the last READY: yes or no off a turn: the text without it,
// and "yes", "no" or "" when it had none.
func Ready(text string) (string, string) {
	locs := readyLine.FindAllStringSubmatchIndex(text, -1)
	if len(locs) == 0 {
		return strings.TrimSpace(text), ""
	}
	l := locs[len(locs)-1]
	return strings.TrimSpace(text[:l[0]] + text[l[1]:]), strings.ToLower(text[l[2]:l[3]])
}

// Summary is a room as a list row shows it.
type Summary struct {
	Room
	Sessions map[string]string // rush session id: member name
	Running  bool
	Paused   bool
	Speaking []string // whose turns are under way
	Over     string   // why it ended, once it has
	Verdict  bool
	Final    string            // the verdict: each member's final position, a line each
	Round    int               // the round under way, or the last one
	Last     map[string]string // a member: the first line of what it said last
	Updated  time.Time
}

// Lister lists rooms again and again, reading only the logs that grew.
type Lister struct {
	seen map[string]listed
}

type listed struct {
	size int64
	s    Summary
}

// List is every room, newest first. Only one goroutine may call it at once.
func (l *Lister) List() []Summary {
	if l.seen == nil {
		l.seen = map[string]listed{}
	}
	ents, _ := os.ReadDir(Root())
	var out []Summary
	for _, e := range ents {
		id := e.Name()
		st, err := os.Stat(logPath(id))
		if err != nil {
			continue
		}
		c, ok := l.seen[id]
		if !ok || c.size != st.Size() {
			r, err := Load(id)
			if err != nil {
				continue
			}
			es, _, _ := Read(id, 0)
			c = listed{size: st.Size(), s: Summarise(r, es)}
			l.seen[id] = c
		}
		s := c.s
		s.Running = s.Over == "" && Running(id)
		out = append(out, s)
	}
	slices.SortFunc(out, func(a, b Summary) int { return b.Created.Compare(a.Created) })
	return out
}

// Summarise is room r as its log es says it stands.
func Summarise(r Room, es []Entry) Summary {
	s := Summary{Room: r, Sessions: map[string]string{}, Last: map[string]string{}, Updated: r.Created}
	speaking := map[string]bool{}
	for _, e := range es {
		s.Updated = e.At
		switch e.Kind {
		case Joined:
			s.Sessions[e.ID] = e.From
		case Turn:
			speaking[e.From] = true
			s.Round = max(s.Round, e.Round)
		case Note:
			s.Last[e.From] = firstLine(e.Text)
		case Say, Paused:
			speaking[e.From] = false
			if e.Kind == Say && e.From != User {
				s.Last[e.From] = firstLine(e.Text)
			}
		case Control:
			switch e.Text {
			case "pause":
				s.Paused = true
			case "resume", "verdict", "stop":
				s.Paused = false
			}
		case Verdict:
			s.Verdict, s.Final = true, e.Text
		case End:
			s.Over = e.Text
		}
	}
	for _, m := range r.Members {
		if speaking[m.Name] && !s.Paused && s.Over == "" {
			s.Speaking = append(s.Speaking, m.Name)
		}
	}
	return s
}

// firstLine is s's first line with words on it.
func firstLine(s string) string {
	for l := range strings.Lines(s) {
		if l = strings.TrimSpace(l); l != "" {
			return l
		}
	}
	return ""
}
