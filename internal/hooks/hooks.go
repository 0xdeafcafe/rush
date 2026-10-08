// Package hooks is rush's UI's only way to its plugins, and nothing in it
// can hold the UI up.
//
// The UI never touches the broker's socket. One goroutine here owns it: it
// connects (starting the broker if a plugin is approved), attaches this
// window, sends the events the UI hands it, and turns what the broker
// pushes into messages. From the UI's side every method returns at once:
//
//   - Emit puts an event in a bounded queue, or drops it when that's full.
//   - State is a copy of what plugins add (commands, settings, overview
//     sections, statuses), swapped in whole, read without a lock.
//   - Next, Command, Intercept and SetSetting are tea.Cmds: bubbletea runs
//     them off the UI goroutine, and each has a deadline.
//
// An intercept that doesn't answer in time lets the message go as it was.
package hooks

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json/jsontext"
	"errors"
	"slices"
	"sync"
	"sync/atomic"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// Queue sizes: events waiting to go to the broker, and what the broker
// sent waiting for the UI to take it.
const (
	eventQueue = 256
	inbox      = 64
)

// Client is one rush window's connection to its plugins.
type Client struct {
	id     string
	events chan plugin.UIEvent
	in     chan tea.Msg
	state  atomic.Pointer[plugin.UIState]
	conn   atomic.Pointer[plugin.Conn]
	// Dropped counts events let go because the queue was full.
	Dropped atomic.Int64
	// attaches counts connections made: a new one is a broker that knows
	// nothing yet of what's on the screen.
	attaches atomic.Uint64
	// stateQueued is a StateMsg waiting in in.
	stateQueued atomic.Bool
	start       sync.Once
	stop        chan struct{}
	// dial is how it reaches the broker; a test's fake in tests.
	dial func(plugin.Handler) (*plugin.Conn, error)
}

// New is a client for this window. It does nothing until Start.
func New() *Client {
	var b [4]byte
	_, _ = rand.Read(b[:])
	c := &Client{
		id:     hex.EncodeToString(b[:]),
		events: make(chan plugin.UIEvent, eventQueue),
		in:     make(chan tea.Msg, inbox),
		stop:   make(chan struct{}),
		dial:   dialBroker,
	}
	c.state.Store(&plugin.UIState{})
	return c
}

// Attaches counts the times it has connected to the broker.
func (c *Client) Attaches() uint64 { return c.attaches.Load() }

// ID names this window to plugins.
func (c *Client) ID() string { return c.id }

// StateMsg says what plugins add has changed; State has it.
type StateMsg struct{}

// DoMsg is something a plugin asks this window to do.
type DoMsg struct{ plugin.UIDo }

// Start connects, in the background, and keeps connecting.
func (c *Client) Start() { c.start.Do(func() { go c.run() }) }

// Close stops it.
func (c *Client) Close() {
	select {
	case <-c.stop:
	default:
		close(c.stop)
	}
	if cn := c.conn.Load(); cn != nil {
		_ = cn.Close()
	}
}

// State is what plugins add now. Never nil; don't change it.
func (c *Client) State() *plugin.UIState { return c.state.Load() }

// Wants is whether any plugin would hear an event of this kind, so the UI
// needn't make one nobody hears.
func (c *Client) Wants(kind string) bool {
	if c.conn.Load() == nil {
		return false
	}
	need := plugin.UIEvents
	if len(kind) > 6 && kind[:6] == "input." {
		need = plugin.UIInput
	}
	for _, p := range c.State().Plugins {
		if slices.Contains(p.UI, need) {
			return true
		}
	}
	return false
}

// Intercepts is whether any plugin is asked before a message goes.
func (c *Client) Intercepts() bool {
	if c.conn.Load() == nil {
		return false
	}
	for _, p := range c.State().Plugins {
		if slices.Contains(p.UI, plugin.UIIntercept) && p.Skipped == "" {
			return true
		}
	}
	return false
}

// Emit hands the broker an event, without waiting: when the queue is full
// the event is dropped and counted.
func (c *Client) Emit(e plugin.UIEvent) {
	if !c.Wants(e.Kind) {
		return
	}
	e.UI = c.id
	if e.At.IsZero() {
		e.At = time.Now()
	}
	select {
	case c.events <- e:
	default:
		c.Dropped.Add(1)
	}
}

// Next waits, off the UI, for the next thing the broker sent: a StateMsg
// or a DoMsg. Run it again after each.
func (c *Client) Next() tea.Cmd {
	return func() tea.Msg {
		select {
		case m := <-c.in:
			if _, ok := m.(StateMsg); ok {
				c.stateQueued.Store(false)
			}
			return m
		case <-c.stop:
			return nil
		}
	}
}

// Command runs a plugin's command, off the UI; done turns its outcome into
// the message the UI gets.
//
// box is whose message box has the keys: a session's id, or "" for the
// Prompt; input is that box, which only a plugin with "input" is shown.
func (c *Client) Command(name, command string, s *plugin.UISession, box string, input *plugin.Box, done func(error) tea.Msg) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		err := c.call(ctx, "ui.command", map[string]any{"plugin": name, "command": command, "ui": c.id, "session": s, "box": box, "input": input}, nil)
		return done(err)
	}
}

// Picked tells a plugin what was chosen from its pick, off the UI.
func (c *Client) Picked(p plugin.Picked, done func(error) tea.Msg) tea.Cmd {
	p.UI = c.id
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		return done(c.call(ctx, "ui.picked", p, nil))
	}
}

// Intercept asks the plugins about a message before it goes, off the UI.
// done always gets an answer: when they can't be asked, or don't answer in
// time, it's "allow".
func (c *Client) Intercept(req plugin.Intercept, done func(plugin.InterceptResult) tea.Msg) tea.Cmd {
	req.UI = c.id
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), plugin.InterceptBudget+150*time.Millisecond)
		defer cancel()
		var r plugin.InterceptResult
		if err := c.call(ctx, "ui.intercept", req, &r); err != nil {
			return done(plugin.InterceptResult{Action: "allow"})
		}
		switch r.Action {
		case "rewrite", "block":
		default:
			r = plugin.InterceptResult{Action: "allow"}
		}
		return done(r)
	}
}

// SetSetting changes a plugin's setting, off the UI.
func (c *Client) SetSetting(name, key, value string, done func(error) tea.Msg) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		return done(c.call(ctx, "ui.settings.set", map[string]string{"plugin": name, "key": key, "value": value}, nil))
	}
}

// Reload tells the running broker an approval changed, or a bundled plugin
// was turned on or off, starting it if nothing ran before. It dials once and returns;
// run it off the UI, as its own tea.Cmd would.
func Reload() error {
	c, err := plugin.DialBroker()
	if err != nil {
		return plugin.EnsureBroker()
	}
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return c.Call(ctx, "reload", nil, nil)
}

var errOffline = errors.New("the plugin broker isn't running")

func (c *Client) call(ctx context.Context, method string, params, out any) error {
	cn := c.conn.Load()
	if cn == nil {
		return errOffline
	}
	return cn.Call(ctx, method, params, out)
}

func dialBroker(h plugin.Handler) (*plugin.Conn, error) {
	if len(plugin.Enabled()) == 0 {
		return nil, errOffline
	}
	cn, err := plugin.DialBrokerWith(h)
	if err == nil {
		return cn, nil
	}
	if err := plugin.EnsureBroker(); err != nil {
		return nil, err
	}
	for range 40 {
		time.Sleep(50 * time.Millisecond)
		if cn, err = plugin.DialBrokerWith(h); err == nil {
			return cn, nil
		}
	}
	return nil, err
}

// run keeps a connection to the broker, and sends it the events.
func (c *Client) run() {
	wait := time.Second
	for {
		select {
		case <-c.stop:
			return
		default:
		}
		cn, err := c.attach()
		if err != nil {
			// Events meanwhile would be stale by the time they went.
			c.drain()
			select {
			case <-c.stop:
				return
			case <-time.After(wait):
			}
			wait = min(wait*2, 30*time.Second)
			continue
		}
		wait = time.Second
		c.pump(cn)
		c.conn.Store(nil)
		c.setState(&plugin.UIState{})
	}
}

// attach connects and says which window this is; the broker answers with
// what plugins add now.
func (c *Client) attach() (*plugin.Conn, error) {
	handler := func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "ui.state":
			var s plugin.UIState
			if err := jsonx.Unmarshal(params, &s); err == nil {
				c.setState(&s)
			}
		case "ui.do":
			var d plugin.UIDo
			if err := jsonx.Unmarshal(params, &d); err == nil && (d.UI == "" || d.UI == c.id) {
				c.push(DoMsg{d})
			}
		}
		return map[string]any{}, nil
	}
	cn, err := c.dial(handler)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var s plugin.UIState
	if err := cn.Call(ctx, "ui.attach", map[string]string{"ui": c.id}, &s); err != nil {
		_ = cn.Close()
		return nil, err
	}
	c.conn.Store(cn)
	c.attaches.Add(1)
	c.setState(&s)
	return cn, nil
}

func (c *Client) pump(cn *plugin.Conn) {
	for {
		select {
		case <-c.stop:
			return
		case <-cn.Done():
			return
		case e := <-c.events:
			if err := cn.Notify("ui.event", e); err != nil {
				_ = cn.Close()
				return
			}
		}
	}
}

func (c *Client) drain() {
	for {
		select {
		case <-c.events:
		default:
			return
		}
	}
}

func (c *Client) setState(s *plugin.UIState) {
	c.state.Store(s)
	c.push(StateMsg{})
}

// push hands the UI a message without waiting. One StateMsg waiting says
// all there is to say, so another isn't queued behind it.
func (c *Client) push(m tea.Msg) {
	if _, ok := m.(StateMsg); ok && !c.stateQueued.CompareAndSwap(false, true) {
		return
	}
	select {
	case c.in <- m:
	default:
		if _, ok := m.(StateMsg); ok {
			c.stateQueued.Store(false)
		}
	}
}

// Static is a client that never connects, holding s: what plugins add, for
// tests and a --render of the screen.
func Static(s plugin.UIState) *Client {
	c := New()
	c.state.Store(&s)
	return c
}
