package drafts

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Manifest: Manifest, Run: Run}) }

// Manifest is the drafts plugin's.
var Manifest = plugin.Manifest{
	Name: "drafts",
	Description: "Keeps a message box until it's sent, even across rush restarts; sets a message aside while you send another; " +
		"and puts back what you set aside, sent, cleared or replaced. Keeps it all in its data folder.",
	Command: []string{"rush"},
	UI:      []string{plugin.UIEvents, plugin.UIInput, plugin.UINotify},
	// No keys of its own: rush gives these its stash and history keys,
	// ctrl+p and ctrl+r, and its #stash.
	Commands: []plugin.CommandSpec{
		{Name: "stash", Description: "set the message aside, or bring it back; it comes back by itself once you send"},
		{Name: "history", Description: "what you set aside, sent, cleared and replaced, to put back in the box"},
	},
	Settings: []plugin.SettingSpec{
		{Key: "called", Title: "Called", Type: "choice", Choices: Called, Default: Called[0],
			Description: "What it's called wherever rush shows it: Stash, or Drafts."},
		{Key: "keep", Title: "Kept of each kind", Type: "choice", Choices: []string{"100", "300", "1000"}, Default: "300",
			Description: "How many sent, cleared and replaced messages it keeps; the oldest go first."},
	},
	MemoryMB: 64,
}

// saveAfter is how long it waits after the last change to write.
const saveAfter = 300 * time.Millisecond

type app struct {
	conn  *plugin.Conn
	ready chan struct{} // closed once conn is set

	mu   sync.Mutex
	book *Book
	path string
	due  *time.Timer

	smu sync.Mutex // one write at a time
}

// Run is the plugin, on its connection to the broker.
func Run(rw io.ReadWriteCloser) error {
	a := &app{ready: make(chan struct{})}
	a.conn = plugin.NewConn(rw, a.handle)
	close(a.ready)
	<-a.conn.Done()
	a.save()
	return nil
}

type event struct {
	Kind    string            `json:"kind"`
	UI      string            `json:"ui"`
	Session *plugin.UISession `json:"session"`
	Text    string            `json:"text"`
	Box     *plugin.Box       `json:"box"`
}

func (e event) key() (key, name string) {
	if e.Session == nil {
		return "", ""
	}
	return e.Session.ID, e.Session.Name
}

func (a *app) handle(_ context.Context, method string, params jsontext.Value) (any, error) {
	<-a.ready
	switch method {
	case "initialize":
		var in struct {
			DataDir  string            `json:"dataDir"`
			Settings map[string]string `json:"settings"`
		}
		_ = jsonx.Unmarshal(params, &in)
		a.mu.Lock()
		a.path = filepath.Join(in.DataDir, "drafts.json")
		s, took := load(a.path, in.DataDir)
		a.book = NewBook(s, ParseKeep(in.Settings["keep"]))
		a.book.W = WordsFor(in.Settings["called"])
		if took {
			a.later()
		}
		stashed := make([]string, 0, len(a.book.S.Stashes))
		for k := range a.book.S.Stashes {
			stashed = append(stashed, k)
		}
		a.mu.Unlock()
		// A note from before it restarted went with it.
		go func() {
			for _, k := range stashed {
				a.note(k)
			}
		}()
		return map[string]any{}, nil
	case "ui.settings":
		var in struct {
			Values map[string]string `json:"values"`
		}
		_ = jsonx.Unmarshal(params, &in)
		a.mu.Lock()
		if a.book != nil && a.book.SetKeep(ParseKeep(in.Values["keep"])) {
			a.later()
		}
		renamed := a.book != nil && a.book.W != WordsFor(in.Values["called"])
		if renamed {
			a.book.W = WordsFor(in.Values["called"])
		}
		a.mu.Unlock()
		// The notes on the boxes say the new name.
		if renamed {
			go func() {
				for _, k := range a.stashKeys() {
					a.note(k)
				}
			}()
		}
		return nil, nil
	case "ui.event":
		var ev event
		if jsonx.Unmarshal(params, &ev) == nil {
			a.event(ev)
		}
		return nil, nil
	case "ui.command":
		var in struct {
			Command string            `json:"command"`
			UI      string            `json:"ui"`
			Session *plugin.UISession `json:"session"`
			Box     string            `json:"box"`
			Input   *plugin.Box       `json:"input"`
		}
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		name := ""
		if in.Session != nil && in.Session.ID == in.Box {
			name = in.Session.Name
		}
		return map[string]any{}, a.command(in.Command, in.UI, in.Box, name, in.Input)
	case "ui.picked":
		var in plugin.Picked
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		return map[string]any{}, a.picked(in)
	case "tools.list":
		return map[string]any{"tools": []any{}}, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// event takes one thing that happened. Events come in order on the
// connection's reading goroutine, so anything that calls rush back goes
// on its own.
func (a *app) event(ev event) {
	key, name := ev.key()
	now := time.Now()
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.book == nil {
		return
	}
	switch ev.Kind {
	case plugin.EvInputChanged:
		box := plugin.Box{Text: ev.Text, Cursor: len([]rune(ev.Text))}
		if ev.Box != nil {
			box = *ev.Box
		}
		if a.book.Changed(key, box) {
			go a.note(key)
		}
	case plugin.EvInputSent:
		if set, ok := a.book.Sent(key, name, ev.Text, now); ok {
			go a.set(ev.UI, set)
			go a.note(key)
		}
	case plugin.EvInputCleared:
		a.book.Cleared(key, name, ev.Text, now)
	case plugin.EvSessionOpened:
		if set, ok := a.book.Opened(key); ok {
			go a.set(ev.UI, set)
		}
		return
	default:
		return
	}
	a.later()
}

func (a *app) command(cmd, ui, key, name string, in *plugin.Box) error {
	now := time.Now()
	a.mu.Lock()
	if a.book == nil {
		a.mu.Unlock()
		return nil
	}
	switch cmd {
	case "stash":
		box := plugin.Box{}
		if in != nil {
			box = *in
		}
		r := a.book.Stash(key, name, box, now)
		a.later()
		a.mu.Unlock()
		if r.Set != nil {
			a.set(ui, *r.Set)
		}
		a.note(key)
		a.notify(ui, key, r.Said)
		return nil
	case "history":
		p := a.book.Pick(key, now)
		a.mu.Unlock()
		return a.conn.Call(context.Background(), "ui.pick", map[string]any{"ui": ui, "pick": p}, nil)
	}
	a.mu.Unlock()
	return nil
}

func (a *app) picked(p plugin.Picked) error {
	now := time.Now()
	a.mu.Lock()
	if a.book == nil {
		a.mu.Unlock()
		return nil
	}
	a.later()
	switch p.Action {
	case "ctrl+d":
		a.book.Forget(p.Item)
		a.mu.Unlock()
		a.note(p.Box)
		return nil
	case "enter":
		set, said, ok := a.book.PutBack(p.Item, p.Box, "", p.Input, now)
		said = a.book.said(said)
		a.mu.Unlock()
		if ok {
			a.set(p.UI, set)
			a.note(p.Box)
			for _, k := range a.stashKeys() {
				a.note(k)
			}
			a.notify(p.UI, p.Box, said)
		}
		return nil
	}
	a.mu.Unlock()
	return nil
}

func (a *app) stashKeys() []string {
	a.mu.Lock()
	defer a.mu.Unlock()
	var out []string
	for k := range a.book.S.Stashes {
		out = append(out, k)
	}
	return out
}

// set sets a box in rush, only while it holds what the book expects.
func (a *app) set(ui string, s Set) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = a.conn.Call(ctx, "ui.input.set", map[string]any{"ui": ui, "session": s.Key, "box": s.Box, "if": s.If}, nil)
}

// note says on a box's edge whether it has a stash.
func (a *app) note(key string) {
	a.mu.Lock()
	text := ""
	if a.book != nil {
		text = a.book.Note(key)
	}
	a.mu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = a.conn.Call(ctx, "ui.box.note", map[string]any{"session": key, "text": text, "tone": "warn"}, nil)
}

func (a *app) notify(ui, key, text string) {
	if text == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = a.conn.Call(ctx, "ui.notify", map[string]any{"ui": ui, "text": text}, nil)
}

// later writes the store once changes stop for saveAfter. Under a.mu.
func (a *app) later() {
	if a.due != nil {
		a.due.Stop()
	}
	a.due = time.AfterFunc(saveAfter, a.save)
}

// save writes the store whole, to a file renamed over the last.
func (a *app) save() {
	a.smu.Lock()
	defer a.smu.Unlock()
	a.mu.Lock()
	if a.book == nil || a.path == "" {
		a.mu.Unlock()
		return
	}
	b, err := jsonx.Marshal(a.book.S)
	path := a.path
	a.mu.Unlock()
	if err != nil {
		return
	}
	tmp := path + ".tmp"
	if os.WriteFile(tmp, b, 0o600) == nil {
		_ = os.Rename(tmp, path)
	}
}

// load reads the store, and once takes in the drafts rush kept itself
// before they were a plugin, saying whether it did. Their file is left as
// it is.
func load(path, dataDir string) (s Store, took bool) {
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, &s)
	}
	if s.Imported {
		return s, false
	}
	// The data folder is <rush's folder>/plugin-data/drafts.
	old, err := os.ReadFile(filepath.Join(filepath.Dir(filepath.Dir(dataDir)), "drafts.json"))
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return s, false // unreadable for now: tried again next time
	}
	s.Imported = true
	if err != nil {
		return s, true
	}
	var was []struct {
		Text  string    `json:"text"`
		At    time.Time `json:"at"`
		Agent string    `json:"agent"`
		Name  string    `json:"name"`
		Kind  string    `json:"kind"`
		Sent  bool      `json:"sent"`
	}
	if jsonx.Unmarshal(old, &was) != nil {
		return s, true
	}
	// A store from before Imported may have taken them in already.
	have := map[[2]string]bool{}
	for _, e := range s.History {
		have[[2]string{e.Kind, strings.TrimSpace(Plain(e.Box))}] = true
	}
	for _, d := range was {
		// A draft kept on purpose lands with what put-backs replaced.
		kind := map[string]string{"draft": Replaced, "sent": Sent, "cleared": Cleared}[d.Kind]
		if kind == "" {
			kind = Cleared
			if d.Sent {
				kind = Sent
			}
		}
		if have[[2]string{kind, strings.TrimSpace(d.Text)}] {
			continue
		}
		s.Seq++
		s.History = append(s.History, Entry{ID: "h" + strconv.Itoa(s.Seq), Kind: kind, Box: plugin.Box{Text: d.Text, Cursor: len([]rune(d.Text))},
			Session: d.Agent, Name: d.Name, At: d.At})
	}
	slices.SortStableFunc(s.History, func(x, y Entry) int { return y.At.Compare(x.At) })
	return s, true
}
