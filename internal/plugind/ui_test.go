package plugind

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func testBroker(t *testing.T) *broker {
	t.Helper()
	home, _ := filepath.EvalSymlinks(t.TempDir())
	t.Setenv("RUSH_HOME", home)
	b := &broker{log: log.New(io.Discard, "", 0), plugins: map[string]*runner{}, quit: make(chan struct{})}
	b.ui = newUIHub(b)
	return b
}

// fakePlugin is a plugin as the broker sees one running: a connection, with
// the plugin's end answering as h says.
type fakePlugin struct {
	r      *runner
	conn   *plugin.Conn // the plugin's end
	events chan plugin.UIEvent
	other  chan string // other notifications, method then params
}

func addPlugin(t *testing.T, b *broker, m *plugin.Manifest, h plugin.Handler) *fakePlugin {
	t.Helper()
	if m.Command == nil {
		m.Command = []string{"bin"}
	}
	mine, theirs := net.Pipe()
	f := &fakePlugin{events: make(chan plugin.UIEvent, 2000), other: make(chan string, 100)}
	f.conn = plugin.NewConn(theirs, func(ctx context.Context, method string, params jsontext.Value) (any, error) {
		if method == "ui.event" {
			var e plugin.UIEvent
			_ = jsonx.Unmarshal(params, &e)
			f.events <- e
			return nil, nil
		}
		if h != nil {
			return h(ctx, method, params)
		}
		f.other <- method + " " + string(params)
		return map[string]any{}, nil
	})
	f.r = newRunner(b, m.Name, "")
	bc := plugin.NewConn(mine, f.r.fromPlugin)
	f.r.mu.Lock()
	f.r.p, f.r.conn, f.r.out, f.r.state = plugin.Plugin{Manifest: *m}, bc, newOutbox(bc, pluginQueue), "running"
	f.r.mu.Unlock()
	b.mu.Lock()
	b.plugins[m.Name] = f.r
	b.mu.Unlock()
	t.Cleanup(func() { f.conn.Close(); bc.Close() })
	return f
}

func (f *fakePlugin) call(method string, params, out any) error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	return f.conn.Call(ctx, method, params, out)
}

// fakeUI is a rush window attached to the broker.
type fakeUI struct {
	conn   *plugin.Conn
	states chan plugin.UIState
	dos    chan plugin.UIDo
}

func attachUI(t *testing.T, b *broker, name string) *fakeUI {
	t.Helper()
	mine, theirs := net.Pipe()
	b.accept(mine)
	u := &fakeUI{states: make(chan plugin.UIState, 100), dos: make(chan plugin.UIDo, 100)}
	u.conn = plugin.NewConn(theirs, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "ui.state":
			var st plugin.UIState
			_ = jsonx.Unmarshal(params, &st)
			u.states <- st
		case "ui.do":
			var d plugin.UIDo
			_ = jsonx.Unmarshal(params, &d)
			u.dos <- d
		}
		return nil, nil
	})
	t.Cleanup(func() { u.conn.Close() })
	if err := u.call("ui.attach", map[string]any{"ui": name}, nil); err != nil {
		t.Fatal(err)
	}
	return u
}

func (u *fakeUI) call(method string, params, out any) error {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return u.conn.Call(ctx, method, params, out)
}

// events sends events, and returns once the broker has taken them.
func (u *fakeUI) events(t *testing.T, evs ...plugin.UIEvent) {
	t.Helper()
	for _, e := range evs {
		if err := u.conn.Notify("ui.event", e); err != nil {
			t.Fatal(err)
		}
	}
	// Notifications are taken in order, before a later request.
	if err := u.call("ui.attach", map[string]any{"ui": "main"}, nil); err != nil {
		t.Fatal(err)
	}
}

func (u *fakeUI) waitState(t *testing.T, what string, ok func(plugin.UIState) bool) plugin.UIState {
	t.Helper()
	deadline := time.After(3 * time.Second)
	for {
		select {
		case st := <-u.states:
			if ok(st) {
				return st
			}
		case <-deadline:
			t.Fatalf("no ui.state with %s", what)
		}
	}
}

func (u *fakeUI) waitDo(t *testing.T) plugin.UIDo {
	t.Helper()
	select {
	case d := <-u.dos:
		return d
	case <-time.After(3 * time.Second):
		t.Fatal("no ui.do")
	}
	return plugin.UIDo{}
}

func sess(id, cwd string) *plugin.UISession { return &plugin.UISession{ID: id, Cwd: cwd, Name: id} }

func TestUIEventsGoOnlyWhereTheyMay(t *testing.T) {
	b := testBroker(t)
	ev := addPlugin(t, b, &plugin.Manifest{Name: "ev", UI: []string{plugin.UIEvents}}, nil)
	in := addPlugin(t, b, &plugin.Manifest{Name: "in", UI: []string{plugin.UIInput}}, nil)
	none := addPlugin(t, b, &plugin.Manifest{Name: "none", UI: []string{plugin.UIOverview}}, nil)
	u := attachUI(t, b, "main")

	s := sess("s1", "/tmp")
	u.events(t,
		plugin.UIEvent{Kind: plugin.EvTurnStarted, UI: "main", Session: s, Text: "leaked"},
		plugin.UIEvent{Kind: plugin.EvInputChanged, UI: "main", Session: s, Text: "my secret"},
		plugin.UIEvent{Kind: plugin.EvTurnEnded, UI: "main", Session: s},
	)
	got := func(f *fakePlugin, n int) []plugin.UIEvent {
		var out []plugin.UIEvent
		for range n {
			select {
			case e := <-f.events:
				out = append(out, e)
			case <-time.After(2 * time.Second):
				t.Fatalf("%s got %d of %d events", f.r.name, len(out), n)
			}
		}
		return out
	}
	evs := got(ev, 2)
	if evs[0].Kind != plugin.EvTurnStarted || evs[0].Text != "" || evs[0].Session.ID != "s1" || evs[1].Kind != plugin.EvTurnEnded {
		t.Fatalf("events plugin got %+v", evs)
	}
	ins := got(in, 1)
	if ins[0].Kind != plugin.EvInputChanged || ins[0].Text != "my secret" {
		t.Fatalf("input plugin got %+v", ins)
	}
	time.Sleep(100 * time.Millisecond)
	for _, f := range []*fakePlugin{ev, in, none} {
		select {
		case e := <-f.events:
			t.Fatalf("%s heard %+v", f.r.name, e)
		default:
		}
	}
}

func TestUICallsNeedTheirCapability(t *testing.T) {
	b := testBroker(t)
	f := addPlugin(t, b, &plugin.Manifest{Name: "bare", Commands: []plugin.CommandSpec{{Name: "go", Description: "Go."}}}, nil)
	for method, params := range map[string]any{
		"ui.overview.set": map[string]any{"session": "s1", "sections": []any{}},
		"ui.status.set":   map[string]any{"session": "s1", "text": "hi"},
		"ui.notify":       map[string]any{"text": "hi"},
		"ui.input.set":    map[string]any{"ui": "main", "session": "s1", "text": "hi"},
		"ui.send":         map[string]any{"session": "s1", "text": "hi"},
	} {
		if err := f.call(method, params, nil); !denied(err) {
			t.Errorf("%s without its capability: %v", method, err)
		}
	}
	var out struct {
		Values map[string]string `json:"values"`
	}
	if err := f.call("ui.settings.get", nil, &out); err != nil || out.Values == nil {
		t.Fatalf("ui.settings.get = %v, %v", out, err)
	}
}

func TestUIOverviewGoesWithThePlugin(t *testing.T) {
	b := testBroker(t)
	f := addPlugin(t, b, &plugin.Manifest{Name: "ov", UI: []string{plugin.UIOverview}}, nil)
	u := attachUI(t, b, "main")

	sections := []plugin.Section{{ID: "ci", Title: "CI", Lines: []plugin.Line{{Text: "\x1b[31mred\x1b[0m", Tone: "bad", URL: "javascript:x"}}}}
	if err := f.call("ui.overview.set", map[string]any{"session": "s1", "sections": sections}, nil); err != nil {
		t.Fatal(err)
	}
	if err := f.call("ui.status.set", map[string]any{"session": "s1", "text": "passing", "tone": "loud"}, nil); err != nil {
		t.Fatal(err)
	}
	st := u.waitState(t, "the section and status", func(st plugin.UIState) bool {
		return len(st.Sections["s1"]) == 1 && len(st.Statuses["s1"]) == 1
	})
	l := st.Sections["s1"][0].Lines[0]
	if st.Sections["s1"][0].Plugin != "ov" || strings.Contains(l.Text, "\x1b") || l.URL != "" || l.Tone != "bad" {
		t.Fatalf("section = %+v", st.Sections["s1"][0])
	}
	if s := st.Statuses["s1"][0]; s.Text != "passing" || s.Tone != "" {
		t.Fatalf("status = %+v", s)
	}
	if len(st.Plugins) != 1 || st.Plugins[0].Name != "ov" {
		t.Fatalf("plugins = %+v", st.Plugins)
	}
	if err := f.call("ui.overview.set", map[string]any{"session": "s1", "sections": make([]plugin.Section, plugin.MaxSections+1)}, nil); err == nil {
		t.Fatal("took too many sections")
	}

	f.r.down(f.r.conn)
	u.waitState(t, "nothing of the stopped plugin", func(st plugin.UIState) bool {
		return len(st.Sections) == 0 && len(st.Statuses) == 0 && len(st.Plugins) == 0
	})
}

func TestUIOverviewClears(t *testing.T) {
	b := testBroker(t)
	f := addPlugin(t, b, &plugin.Manifest{Name: "ov", UI: []string{plugin.UIOverview}}, nil)
	u := attachUI(t, b, "main")
	_ = f.call("ui.overview.set", map[string]any{"session": "s1", "sections": []plugin.Section{{ID: "a", Title: "A"}}}, nil)
	u.waitState(t, "the section", func(st plugin.UIState) bool { return len(st.Sections["s1"]) == 1 })
	_ = f.call("ui.overview.set", map[string]any{"session": "s1", "sections": []plugin.Section{}}, nil)
	u.waitState(t, "no section", func(st plugin.UIState) bool { return len(st.Sections) == 0 })
}

func TestUIIntercept(t *testing.T) {
	b := testBroker(t)
	intercept := []string{plugin.UIInput, plugin.UIIntercept}
	var asked atomic.Int32
	answer := func(f func(plugin.Intercept) plugin.InterceptResult) plugin.Handler {
		return func(_ context.Context, method string, params jsontext.Value) (any, error) {
			if method != "ui.intercept" {
				return map[string]any{}, nil
			}
			var in plugin.Intercept
			_ = jsonx.Unmarshal(params, &in)
			return f(in), nil
		}
	}
	addPlugin(t, b, &plugin.Manifest{Name: "a", UI: intercept}, answer(func(in plugin.Intercept) plugin.InterceptResult {
		return plugin.InterceptResult{Action: "rewrite", Text: in.Text + " [a]"}
	}))
	addPlugin(t, b, &plugin.Manifest{Name: "b", UI: intercept}, answer(func(in plugin.Intercept) plugin.InterceptResult {
		return plugin.InterceptResult{Action: "rewrite", Text: strings.ToUpper(in.Text)} // sees a's
	}))
	addPlugin(t, b, &plugin.Manifest{Name: "c", UI: intercept}, answer(func(in plugin.Intercept) plugin.InterceptResult {
		if strings.Contains(in.Text, "STOP") {
			return plugin.InterceptResult{Action: "block", Reason: "no\x1b stopping"}
		}
		return plugin.InterceptResult{Action: "allow"}
	}))
	addPlugin(t, b, &plugin.Manifest{Name: "d", UI: []string{plugin.UIInput}}, answer(func(plugin.Intercept) plugin.InterceptResult {
		asked.Add(1)
		return plugin.InterceptResult{Action: "block"}
	}))
	u := attachUI(t, b, "main")

	var res plugin.InterceptResult
	if err := u.call("ui.intercept", plugin.Intercept{Hook: "before-send", UI: "main", Text: "hello"}, &res); err != nil {
		t.Fatal(err)
	}
	if res.Action != "rewrite" || res.Text != "HELLO [A]" || res.Plugin != "a, b" {
		t.Fatalf("chained = %+v", res)
	}
	if err := u.call("ui.intercept", plugin.Intercept{Hook: "before-send", UI: "main", Text: "stop"}, &res); err != nil {
		t.Fatal(err)
	}
	if res.Action != "block" || res.Plugin != "c" || res.Reason != "no stopping" {
		t.Fatalf("blocked = %+v", res)
	}
	if asked.Load() != 0 {
		t.Fatal("a plugin without intercept was asked")
	}
}

func TestUIInterceptTimeoutsStrikeOut(t *testing.T) {
	b := testBroker(t)
	var asked atomic.Int32
	addPlugin(t, b, &plugin.Manifest{Name: "slow", UI: []string{plugin.UIInput, plugin.UIIntercept}},
		func(context.Context, string, jsontext.Value) (any, error) {
			asked.Add(1)
			time.Sleep(time.Second)
			return plugin.InterceptResult{Action: "block"}, nil
		})
	u := attachUI(t, b, "main")
	for i := range plugin.InterceptStrikeOut + 1 {
		began := time.Now()
		var res plugin.InterceptResult
		if err := u.call("ui.intercept", plugin.Intercept{Text: "hi"}, &res); err != nil {
			t.Fatal(err)
		}
		if res.Action != "allow" {
			t.Fatalf("a plugin that didn't answer held it: %+v", res)
		}
		if took := time.Since(began); took > plugin.InterceptBudget+150*time.Millisecond {
			t.Fatalf("call %d took %s", i, took)
		}
	}
	if n := asked.Load(); n != plugin.InterceptStrikeOut {
		t.Fatalf("asked %d times, want %d before it's skipped", n, plugin.InterceptStrikeOut)
	}
	u.waitState(t, "slow skipped", func(st plugin.UIState) bool {
		return len(st.Plugins) == 1 && st.Plugins[0].Skipped != ""
	})
	// Restarted, it's asked again.
	b.ui.gone("slow")
	_ = u.call("ui.intercept", plugin.Intercept{Text: "hi"}, nil)
	if asked.Load() != plugin.InterceptStrikeOut+1 {
		t.Fatal("not asked again after a restart")
	}
}

func TestUISendReachesOnlyItsWorkspaces(t *testing.T) {
	b := testBroker(t)
	ws, _ := filepath.EvalSymlinks(t.TempDir())
	elsewhere, _ := filepath.EvalSymlinks(t.TempDir())
	_ = os.MkdirAll(filepath.Join(ws, "app"), 0o700)
	f := addPlugin(t, b, &plugin.Manifest{Name: "s", UI: []string{plugin.UIEvents, plugin.UISend, plugin.UINotify, plugin.UIInput},
		Workspaces: []string{ws}}, nil)
	u := attachUI(t, b, "main")
	u.events(t,
		plugin.UIEvent{Kind: plugin.EvSessionOpened, UI: "main", Session: sess("in", filepath.Join(ws, "app"))},
		plugin.UIEvent{Kind: plugin.EvSessionOpened, UI: "main", Session: sess("out", elsewhere)},
	)
	if err := f.call("ui.send", map[string]any{"session": "in", "text": "continue"}, nil); err != nil {
		t.Fatal(err)
	}
	if d := u.waitDo(t); d.Kind != "send" || d.Session != "in" || d.Text != "continue" || d.Plugin != "s" {
		t.Fatalf("ui.do = %+v", d)
	}
	for _, id := range []string{"out", "never-seen"} {
		if err := f.call("ui.send", map[string]any{"session": id, "text": "continue"}, nil); !denied(err) {
			t.Errorf("sent to %s: %v", id, err)
		}
	}
	if err := f.call("ui.send", map[string]any{"session": "in", "text": "  "}, nil); err == nil || denied(err) {
		t.Errorf("sent nothing: %v", err)
	}
	// It can't send without limit.
	var err error
	sent := 1
	for range sendsPerMin {
		if err = f.call("ui.send", map[string]any{"session": "in", "text": "again"}, nil); err != nil {
			break
		}
		sent++
	}
	if !limited(err) || sent != sendsPerMin {
		t.Fatalf("sent %d in a minute: %v", sent, err)
	}
	for range sent - 1 {
		u.waitDo(t)
	}

	// Notices are cleaned and limited too.
	for i := range notifyBurst {
		if err := f.call("ui.notify", map[string]any{"text": fmt.Sprint("hi\x1b", i), "tone": "good"}, nil); err != nil {
			t.Fatal(err)
		}
	}
	if err := f.call("ui.notify", map[string]any{"text": "one more"}, nil); !limited(err) {
		t.Fatalf("notices without limit: %v", err)
	}
	if d := u.waitDo(t); d.Kind != "notify" || d.Text != "hi0" || d.Tone != "good" {
		t.Fatalf("notice = %+v", d)
	}
	if err := f.call("ui.input.set", map[string]any{"ui": "main", "session": "in", "text": "draft"}, nil); err != nil {
		t.Fatal(err)
	}
}

func TestUISlowPluginHoldsUpNoOne(t *testing.T) {
	b := testBroker(t)
	release := make(chan struct{})
	var slowGot []plugin.UIEvent
	done := make(chan struct{})
	addPlugin(t, b, &plugin.Manifest{Name: "a-slow", UI: []string{plugin.UIEvents}}, nil)
	slow := b.runner("a-slow")
	// Replace its end with one that stops reading at the first event.
	mine, theirs := net.Pipe()
	var n atomic.Int32
	pc := plugin.NewConn(theirs, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		if n.Add(1) == 1 {
			<-release
		}
		var e plugin.UIEvent
		_ = jsonx.Unmarshal(params, &e)
		slowGot = append(slowGot, e)
		if e.Kind == plugin.EvNetworkUp {
			close(done)
		}
		return nil, nil
	})
	bc := plugin.NewConn(mine, slow.fromPlugin)
	slow.mu.Lock()
	slow.out.close()
	slow.conn, slow.out = bc, newOutbox(bc, pluginQueue)
	slow.mu.Unlock()
	t.Cleanup(func() { pc.Close(); bc.Close() })
	fast := addPlugin(t, b, &plugin.Manifest{Name: "b-fast", UI: []string{plugin.UIEvents}}, nil)
	u := attachUI(t, b, "main")

	const total = 1000
	began := time.Now()
	for i := range total - 1 {
		_ = u.conn.Notify("ui.event", plugin.UIEvent{Kind: plugin.EvTurnStarted, UI: "main", Session: sess(fmt.Sprint("s", i), "/tmp")})
	}
	_ = u.conn.Notify("ui.event", plugin.UIEvent{Kind: plugin.EvNetworkUp, UI: "main"})
	if took := time.Since(began); took > 2*time.Second {
		t.Fatalf("handing over events took %s with a plugin stuck", took)
	}
	deadline := time.After(3 * time.Second)
	for last := false; !last; {
		select {
		case e := <-fast.events:
			last = e.Kind == plugin.EvNetworkUp
		case <-deadline:
			t.Fatal("the fast plugin never heard the last event")
		}
	}
	close(release)
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		t.Fatal("the slow plugin never caught up")
	}
	if len(slowGot) > pluginQueue+4 {
		t.Fatalf("slow plugin got %d events: its queue isn't bounded", len(slowGot))
	}
}

func TestOutboxCoalescesAndDropsOldest(t *testing.T) {
	o := &outbox{max: 3, wake: make(chan struct{}, 1), done: make(chan struct{})}
	o.put("ui.event", "a", "k")
	o.put("ui.event", "b", "")
	o.put("ui.event", "c", "k") // replaces a, goes last
	if len(o.q) != 2 || string(o.q[0].params) != `"b"` || string(o.q[1].params) != `"c"` {
		t.Fatalf("q = %+v", o.q)
	}
	o.put("ui.event", "d", "")
	o.put("ui.event", "e", "")
	if len(o.q) != 3 || string(o.q[0].params) != `"c"` || o.dropped != 1 {
		t.Fatalf("q = %+v, dropped %d", o.q, o.dropped)
	}
}

func TestUICommandsAndSettings(t *testing.T) {
	b := testBroker(t)
	m := plugin.Manifest{Name: "cmd", Command: []string{"bin"},
		Commands: []plugin.CommandSpec{{Name: "go", Description: "Go."}},
		Settings: []plugin.SettingSpec{{Key: "on", Title: "On", Type: "bool"}}}
	dir := filepath.Join(plugin.Root(), m.Name)
	_ = os.MkdirAll(dir, 0o700)
	mb, _ := jsonx.Marshal(m)
	_ = os.WriteFile(filepath.Join(dir, "plugin.json"), mb, 0o600)
	p, err := plugin.Load(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := plugin.Approve(p); err != nil {
		t.Fatal(err)
	}
	f := addPlugin(t, b, &m, nil)
	u := attachUI(t, b, "main")

	if err := u.call("ui.command", map[string]any{"plugin": "cmd", "command": "go", "ui": "main", "session": sess("s1", "/tmp")}, nil); err != nil {
		t.Fatal(err)
	}
	if got := <-f.other; !strings.HasPrefix(got, "ui.command ") || !strings.Contains(got, `"s1"`) {
		t.Fatalf("plugin got %s", got)
	}
	if err := u.call("ui.command", map[string]any{"plugin": "cmd", "command": "rm"}, nil); err == nil {
		t.Fatal("ran a command it doesn't declare")
	}

	if err := u.call("ui.settings.set", map[string]any{"plugin": "cmd", "key": "on", "value": "maybe"}, nil); err == nil {
		t.Fatal("took a bad value")
	}
	if err := u.call("ui.settings.set", map[string]any{"plugin": "cmd", "key": "on", "value": "true"}, nil); err != nil {
		t.Fatal(err)
	}
	select {
	case got := <-f.other:
		if got != `ui.settings {"values":{"on":"true"}}` {
			t.Fatalf("plugin got %s", got)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("plugin wasn't told its settings")
	}
	u.waitState(t, "the new value", func(st plugin.UIState) bool {
		return len(st.Plugins) == 1 && st.Plugins[0].Values["on"] == "true" && len(st.Plugins[0].Commands) == 1
	})

	// initialize tells it its ui, commands and settings.
	mine, theirs := net.Pipe()
	got := make(chan jsontext.Value, 1)
	pc := plugin.NewConn(theirs, func(_ context.Context, _ string, params jsontext.Value) (any, error) {
		got <- params
		return map[string]any{}, nil
	})
	bc := plugin.NewConn(mine, nil)
	defer pc.Close()
	defer bc.Close()
	if err := f.r.handshake(p, bc); err != nil {
		t.Fatal(err)
	}
	var init struct {
		Commands []string          `json:"commands"`
		Settings map[string]string `json:"settings"`
	}
	_ = jsonx.Unmarshal(<-got, &init)
	if len(init.Commands) != 1 || init.Commands[0] != "go" || init.Settings["on"] != "true" {
		t.Fatalf("initialize = %+v", init)
	}
}

// limited is a call allowed, but made too often.
func limited(err error) bool {
	var e *plugin.Error
	return errors.As(err, &e) && e.Code == plugin.CodeLimited
}

// A plugin with "input" sets a box whole, only if it still holds what it
// expects, and puts a note on its edge; a command shows the box only to a
// plugin that may see it.
func TestUIBoxWhole(t *testing.T) {
	b := testBroker(t)
	f := addPlugin(t, b, &plugin.Manifest{Name: "box", UI: []string{plugin.UIInput},
		Commands: []plugin.CommandSpec{{Name: "go", Description: "Go."}}}, nil)
	g := addPlugin(t, b, &plugin.Manifest{Name: "blind",
		Commands: []plugin.CommandSpec{{Name: "go", Description: "Go."}}}, nil)
	u := attachUI(t, b, "main")

	was := "draft"
	box := map[string]any{"text": "see [Image #1] [Pasted text #2 +4 lines]", "cursor": 3,
		"pastes": map[string]string{"2": "a\nb\nc\nd"}, "images": map[string]string{"1": "/tmp/a.png"}}
	if err := f.call("ui.input.set", map[string]any{"ui": "main", "session": "s1", "box": box, "if": was}, nil); err != nil {
		t.Fatal(err)
	}
	d := u.waitDo(t)
	if d.Kind != "input.set" || d.Box == nil || d.Box.Cursor != 3 || d.Box.Images[1] != "/tmp/a.png" || d.Box.Pastes[2] == "" || d.If == nil || *d.If != was {
		t.Fatalf("do: %+v", d)
	}
	bad := map[string]any{"text": "x", "images": map[string]string{"1": "a.png"}}
	if err := f.call("ui.input.set", map[string]any{"ui": "main", "session": "s1", "box": bad}, nil); err == nil {
		t.Fatal("an image that isn't an absolute path was taken")
	}

	if err := f.call("ui.box.note", map[string]any{"session": "", "text": "stashed · alt+s brings it back", "tone": "warn"}, nil); err != nil {
		t.Fatal(err)
	}
	u.waitState(t, "the Prompt's note", func(st plugin.UIState) bool {
		n := st.Notes[""]
		return len(n) == 1 && n[0].Plugin == "box" && n[0].Tone == "warn"
	})
	if err := g.call("ui.box.note", map[string]any{"session": "s1", "text": "hi"}, nil); err == nil {
		t.Fatal("a plugin without input put a note on a box")
	}

	in := map[string]any{"text": "secret", "cursor": 6}
	for _, name := range []string{"box", "blind"} {
		if err := u.call("ui.command", map[string]any{"plugin": name, "command": "go", "ui": "main", "box": "s1", "input": in}, nil); err != nil {
			t.Fatal(err)
		}
	}
	if got := <-f.other; !strings.Contains(got, `"secret"`) {
		t.Fatalf("the plugin with input wasn't shown the box: %s", got)
	}
	if got := <-g.other; strings.Contains(got, "secret") || !strings.Contains(got, `"box":"s1"`) {
		t.Fatalf("the plugin without input: %s", got)
	}
}

// A plugin shows a pick only in answer to its command in that window, and
// hears what was chosen.
func TestUIPickAnswersACommand(t *testing.T) {
	b := testBroker(t)
	f := addPlugin(t, b, &plugin.Manifest{Name: "pk", Commands: []plugin.CommandSpec{{Name: "go", Description: "Go."}}}, nil)
	u := attachUI(t, b, "main")
	pick := map[string]any{"id": "history", "title": "Drafts", "tabs": []string{"Sent", "Cleared"},
		"items":   []map[string]any{{"id": "a", "text": "one\x1b[31m\ntwo", "tab": 1}},
		"actions": []map[string]any{{"key": "enter", "name": "put back"}, {"key": "ctrl+d", "name": "forget", "stay": true}}}
	if err := f.call("ui.pick", map[string]any{"ui": "main", "pick": pick}, nil); err == nil {
		t.Fatal("a pick shown out of the blue")
	}
	if err := u.call("ui.command", map[string]any{"plugin": "pk", "command": "go", "ui": "main"}, nil); err != nil {
		t.Fatal(err)
	}
	<-f.other
	if err := f.call("ui.pick", map[string]any{"ui": "other", "pick": pick}, nil); err == nil {
		t.Fatal("a pick shown in a window that didn't ask")
	}
	if err := f.call("ui.pick", map[string]any{"ui": "main", "pick": pick}, nil); err != nil {
		t.Fatal(err)
	}
	d := u.waitDo(t)
	if d.Kind != "pick" || d.Pick == nil || d.Pick.ID != "history" || strings.Contains(d.Pick.Items[0].Text, "\x1b") || !strings.Contains(d.Pick.Items[0].Text, "\n") {
		t.Fatalf("do: %+v", d)
	}
	badKey := map[string]any{"id": "x", "title": "X", "items": []any{}, "actions": []map[string]any{{"key": "q", "name": "quit"}}}
	if err := f.call("ui.pick", map[string]any{"ui": "main", "pick": badKey}, nil); err == nil {
		t.Fatal("an action on a key that types was taken")
	}
	if err := u.call("ui.picked", map[string]any{"plugin": "pk", "pick": "history", "item": "a", "action": "enter", "ui": "main", "input": map[string]any{"text": "secret"}}, nil); err != nil {
		t.Fatal(err)
	}
	if got := <-f.other; !strings.HasPrefix(got, "ui.picked ") || !strings.Contains(got, `"item":"a"`) || strings.Contains(got, "secret") {
		t.Fatalf("plugin got %s", got)
	}
}
