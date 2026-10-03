package plugind

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// The broker stands between plugins and rush's screen. rush's UIs attach
// on broker.sock and tell it what happens there; it passes each plugin what
// its manifest lets it hear, and passes back what plugins add, checked and
// cleaned. Nothing here waits on a plugin or a UI while holding anything
// another depends on: every plugin and every UI has its own bounded queue,
// written from its own goroutine, and every call to a plugin has a deadline.

// Limits on the UI side of the broker.
const (
	pluginQueue   = 256 // notifications waiting for one plugin
	uiQueue       = 64  // notifications waiting for one UI
	statePushGap  = 100 * time.Millisecond
	commandWait   = 10 * time.Second
	notifyEvery   = time.Second // a plugin's notices refill at this rate …
	notifyBurst   = 3           // … up to this many
	sendsPerMin   = 10          // messages a plugin sends with ui.send, a minute
	maxSessionKey = 256
)

// uiHub is what the broker holds of rush's screens.
type uiHub struct {
	b *broker

	mu       sync.Mutex
	uis      map[*plugin.Conn]*uiConn
	sessions map[string]plugin.UISession // seen in events, by rush id
	order    []string                    // sessions oldest first, to forget
	sections map[string]map[string][]plugin.Section
	statuses map[string]map[string]plugin.Status
	notes    map[string]map[string]plugin.Status // by session, "" the Prompt
	strikes  map[string]int
	skipped  map[string]string
	notices  map[string]*bucket
	sends    map[string][]time.Time
	// asked is when each plugin was last run by a UI, a command or a
	// choice, by UI: what lets it show that UI a pick.
	asked map[string]map[string]time.Time

	pushing  bool
	lastPush time.Time
}

type uiConn struct {
	name string
	out  *outbox
}

func newUIHub(b *broker) *uiHub {
	return &uiHub{b: b, uis: map[*plugin.Conn]*uiConn{}, sessions: map[string]plugin.UISession{},
		sections: map[string]map[string][]plugin.Section{}, statuses: map[string]map[string]plugin.Status{}, notes: map[string]map[string]plugin.Status{},
		strikes: map[string]int{}, skipped: map[string]string{}, notices: map[string]*bucket{}, sends: map[string][]time.Time{},
		asked: map[string]map[string]time.Time{}}
}

// outbox sends notifications on one connection from its own goroutine, so a
// peer that stops reading holds up nothing but itself. When it's full the
// oldest is dropped; a message with a key replaces the one waiting with the
// same key, and goes last.
type outbox struct {
	mu      sync.Mutex
	q       []outMsg
	max     int
	dropped int
	wake    chan struct{}
	done    chan struct{}
	once    sync.Once
}

type outMsg struct {
	method string
	params jsontext.Value
	key    string
}

func newOutbox(conn *plugin.Conn, max int) *outbox {
	o := &outbox{max: max, wake: make(chan struct{}, 1), done: make(chan struct{})}
	go o.loop(conn)
	return o
}

func (o *outbox) put(method string, params any, key string) {
	if o == nil {
		return
	}
	p, err := jsonx.Marshal(params)
	if err != nil {
		return
	}
	o.mu.Lock()
	if key != "" {
		o.q = slices.DeleteFunc(o.q, func(m outMsg) bool { return m.key == key })
	}
	if len(o.q) >= o.max {
		o.q = slices.Delete(o.q, 0, 1)
		o.dropped++
	}
	o.q = append(o.q, outMsg{method: method, params: p, key: key})
	o.mu.Unlock()
	select {
	case o.wake <- struct{}{}:
	default:
	}
}

func (o *outbox) loop(conn *plugin.Conn) {
	for {
		select {
		case <-o.done:
			return
		case <-conn.Done():
			return
		case <-o.wake:
		}
		for {
			o.mu.Lock()
			if len(o.q) == 0 {
				o.mu.Unlock()
				break
			}
			m := o.q[0]
			o.q = slices.Delete(o.q, 0, 1)
			o.mu.Unlock()
			if conn.Notify(m.method, m.params) != nil {
				return
			}
		}
	}
}

func (o *outbox) close() {
	if o != nil {
		o.once.Do(func() { close(o.done) })
	}
}

// bucket is a token bucket.
type bucket struct {
	tokens float64
	at     time.Time
}

func (k *bucket) take(now time.Time, every time.Duration, burst float64) bool {
	k.tokens = min(burst, k.tokens+now.Sub(k.at).Seconds()/every.Seconds())
	k.at = now
	if k.tokens < 1 {
		return false
	}
	k.tokens--
	return true
}

// callWithin calls a plugin, giving up at ctx's deadline even when the
// plugin has stopped reading and the write itself would wait.
func callWithin(ctx context.Context, conn *plugin.Conn, method string, params any) (jsontext.Value, error) {
	p, err := jsonx.Marshal(params)
	if err != nil {
		return nil, err
	}
	type result struct {
		v   jsontext.Value
		err error
	}
	ch := make(chan result, 1)
	go func() {
		v, err := conn.CallRaw(ctx, method, p)
		ch <- result{v, err}
	}()
	select {
	case r := <-ch:
		return r.v, r.err
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

func badParams(msg string) error { return &plugin.Error{Code: plugin.CodeInvalidParams, Message: msg} }

// fromUI answers a UI's ui.* messages on broker.sock.
func (h *uiHub) fromUI(ctx context.Context, conn *plugin.Conn, method string, params jsontext.Value) (any, error) {
	switch method {
	case "ui.attach":
		var p struct {
			UI string `json:"ui"`
		}
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, badParams(err.Error())
		}
		h.attach(conn, p.UI)
		return h.state(), nil

	case "ui.event":
		var ev plugin.UIEvent
		if err := jsonx.Unmarshal(params, &ev); err != nil {
			return nil, badParams(err.Error())
		}
		h.event(ev)
		return map[string]any{}, nil

	case "ui.command":
		var p struct {
			Plugin  string            `json:"plugin"`
			Command string            `json:"command"`
			UI      string            `json:"ui"`
			Session *plugin.UISession `json:"session,omitempty"`
			// Box is whose message box has the keys: a session's id, or
			// "" for the Prompt; Input is that box, for "input" only.
			Box   string      `json:"box"`
			Input *plugin.Box `json:"input,omitempty"`
		}
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, badParams(err.Error())
		}
		r := h.b.runner(p.Plugin)
		if r == nil {
			return nil, fmt.Errorf("%s is not running", p.Plugin)
		}
		m, conn := r.live()
		if conn == nil {
			return nil, fmt.Errorf("%s is not running", p.Plugin)
		}
		if !slices.ContainsFunc(m.Commands, func(c plugin.CommandSpec) bool { return c.Name == p.Command }) {
			return nil, badParams(p.Plugin + " has no command " + p.Command)
		}
		if !m.CanUI(plugin.UIInput) {
			p.Input = nil
		}
		h.ask(p.Plugin, p.UI)
		ctx, cancel := context.WithTimeout(ctx, commandWait)
		defer cancel()
		if _, err := callWithin(ctx, conn, "ui.command", p); err != nil {
			return nil, err
		}
		return map[string]any{}, nil

	case "ui.picked":
		var p plugin.Picked
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, badParams(err.Error())
		}
		r := h.b.runner(p.Plugin)
		if r == nil {
			return nil, fmt.Errorf("%s is not running", p.Plugin)
		}
		m, conn := r.live()
		if conn == nil {
			return nil, fmt.Errorf("%s is not running", p.Plugin)
		}
		if !m.CanUI(plugin.UIInput) {
			p.Input = nil
		}
		h.ask(p.Plugin, p.UI)
		ctx, cancel := context.WithTimeout(ctx, commandWait)
		defer cancel()
		if _, err := callWithin(ctx, conn, "ui.picked", p); err != nil {
			return nil, err
		}
		return map[string]any{}, nil

	case "ui.intercept":
		var in plugin.Intercept
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, badParams(err.Error())
		}
		return h.intercept(ctx, in), nil

	case "ui.intercept.answer":
		var in plugin.InterceptAnswer
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, badParams(err.Error())
		}
		return h.answer(ctx, in), nil

	case "ui.settings.set":
		var p struct {
			Plugin string `json:"plugin"`
			Key    string `json:"key"`
			Value  string `json:"value"`
		}
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, badParams(err.Error())
		}
		if err := plugin.SetSetting(p.Plugin, p.Key, p.Value); err != nil {
			return nil, badParams(err.Error())
		}
		if r := h.b.runner(p.Plugin); r != nil {
			if m, conn := r.live(); conn != nil {
				r.notify("ui.settings", map[string]any{"values": plugin.SettingValuesOf(&m.Manifest)}, "settings")
			}
		}
		h.changed()
		return map[string]any{}, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// attach takes conn as UI name until it closes.
func (h *uiHub) attach(conn *plugin.Conn, name string) {
	h.mu.Lock()
	if old := h.uis[conn]; old != nil {
		old.name = name
		h.mu.Unlock()
		return
	}
	u := &uiConn{name: name, out: newOutbox(conn, uiQueue)}
	h.uis[conn] = u
	h.mu.Unlock()
	go func() {
		<-conn.Done()
		h.mu.Lock()
		delete(h.uis, conn)
		h.mu.Unlock()
		u.out.close()
	}()
}

// changed pushes the state to every UI soon: at once if it hasn't lately,
// else once statePushGap has passed since the last, however many changes
// came between.
func (h *uiHub) changed() {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.pushing {
		return
	}
	h.pushing = true
	time.AfterFunc(max(0, statePushGap-time.Since(h.lastPush)), h.push)
}

func (h *uiHub) push() {
	h.mu.Lock()
	h.pushing, h.lastPush = false, time.Now()
	h.mu.Unlock()
	st := h.state()
	h.mu.Lock()
	for _, u := range h.uis {
		u.out.put("ui.state", st, "state")
	}
	h.mu.Unlock()
}

// do asks the UI named in d, or every UI, to do something now.
func (h *uiHub) do(d plugin.UIDo) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, u := range h.uis {
		if d.UI == "" || d.UI == u.name {
			u.out.put("ui.do", d, "")
		}
	}
}

// ask notes that a UI just ran one of a plugin's commands, or chose from
// its pick.
func (h *uiHub) ask(name, ui string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.asked[name] == nil {
		h.asked[name] = map[string]time.Time{}
	}
	h.asked[name][ui] = time.Now()
}

// running is every plugin up now, by name.
func (h *uiHub) running() []*runner {
	h.b.mu.Lock()
	rs := make([]*runner, 0, len(h.b.plugins))
	for _, r := range h.b.plugins {
		rs = append(rs, r)
	}
	h.b.mu.Unlock()
	rs = slices.DeleteFunc(rs, func(r *runner) bool { _, c := r.live(); return c == nil })
	sort.Slice(rs, func(i, j int) bool { return rs[i].name < rs[j].name })
	return rs
}

// state is what the UIs draw from.
func (h *uiHub) state() plugin.UIState {
	st := plugin.UIState{Plugins: []plugin.UIPlugin{}}
	for _, r := range h.running() {
		m, _ := r.live()
		if len(m.UI) == 0 && len(m.Commands) == 0 && len(m.Settings) == 0 {
			continue
		}
		st.Plugins = append(st.Plugins, plugin.UIPlugin{Name: r.name, Commands: m.Commands, Settings: m.Settings,
			Values: plugin.SettingValuesOf(&m.Manifest), UI: m.UI})
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	for i := range st.Plugins {
		st.Plugins[i].Skipped = h.skipped[st.Plugins[i].Name]
	}
	if len(h.sections) > 0 {
		st.Sections = map[string][]plugin.UISection{}
		for sess, byPlugin := range h.sections {
			for _, name := range sortedKeys(byPlugin) {
				for _, s := range byPlugin[name] {
					st.Sections[sess] = append(st.Sections[sess], plugin.UISection{Plugin: name, Section: s})
				}
			}
		}
	}
	if len(h.statuses) > 0 {
		st.Statuses = map[string][]plugin.UIStatus{}
		for sess, byPlugin := range h.statuses {
			for _, name := range sortedKeys(byPlugin) {
				st.Statuses[sess] = append(st.Statuses[sess], plugin.UIStatus{Plugin: name, Status: byPlugin[name]})
			}
		}
	}
	if len(h.notes) > 0 {
		st.Notes = map[string][]plugin.UIStatus{}
		for sess, byPlugin := range h.notes {
			for _, name := range sortedKeys(byPlugin) {
				st.Notes[sess] = append(st.Notes[sess], plugin.UIStatus{Plugin: name, Status: byPlugin[name]})
			}
		}
	}
	return st
}

func sortedKeys[V any](m map[string]V) []string {
	ks := make([]string, 0, len(m))
	for k := range m {
		ks = append(ks, k)
	}
	sort.Strings(ks)
	return ks
}

// event remembers the session an event is about, and passes the event on
// to each plugin that may hear it, without waiting for any.
func (h *uiHub) event(ev plugin.UIEvent) {
	if s := ev.Session; s != nil && s.ID != "" && len(s.ID) <= maxSessionKey {
		h.mu.Lock()
		if _, ok := h.sessions[s.ID]; !ok {
			h.order = append(h.order, s.ID)
		}
		h.sessions[s.ID] = *s
		var gone []string
		if n := len(h.order) - plugin.MaxSessionsTracked; n > 0 {
			gone = slices.Clone(h.order[:n])
			h.order = slices.Delete(h.order, 0, n)
		}
		for _, id := range gone {
			delete(h.sessions, id)
		}
		h.mu.Unlock()
	}
	key := ""
	if ev.Kind == plugin.EvInputChanged {
		id := ""
		if ev.Session != nil {
			id = ev.Session.ID
		}
		key = "input.changed\x00" + ev.UI + "\x00" + id
	}
	for _, r := range h.running() {
		m, _ := r.live()
		if e, ok := ev.For(&m.Manifest); ok {
			r.notify("ui.event", e, key)
		}
	}
}

// intercept asks each plugin with "intercept", in name order, about a
// message before it goes. Each has InterceptEach, and all InterceptBudget;
// a plugin that doesn't answer in time lets it go as it is, and one that
// fails to InterceptStrikeOut times in a row isn't asked again until it
// restarts.
func (h *uiHub) intercept(ctx context.Context, in plugin.Intercept) plugin.InterceptResult {
	return h.chain(ctx, in, "", plugin.InterceptResult{})
}

// answer hands the key chosen in a plugin's question to that plugin, which
// has AnswerWait to say what becomes of the message, then asks the
// plugins after it. A plugin that can't answer holds the message back:
// the choice may have been to keep something out of it.
func (h *uiHub) answer(ctx context.Context, in plugin.InterceptAnswer) plugin.InterceptResult {
	held := func(why string) plugin.InterceptResult {
		return plugin.InterceptResult{Action: "block", Plugin: in.Plugin, Reason: plugin.CleanNotice(why)}
	}
	r := h.b.runner(in.Plugin)
	if r == nil {
		return held("it isn't running")
	}
	m, conn := r.live()
	if conn == nil || !m.CanUI(plugin.UIIntercept) {
		return held("it isn't running")
	}
	wait, cancel := context.WithTimeout(ctx, plugin.AnswerWait)
	defer cancel()
	raw, err := callWithin(wait, conn, "ui.intercept.answer", map[string]any{
		"hook": in.Hook, "ui": in.UI, "box": in.Box, "session": in.Session, "text": in.Text, "asks": in.Asks, "id": in.ID, "key": in.Key,
		"value": plugin.CleanInput(in.Value)})
	var res plugin.InterceptResult
	if err == nil {
		err = jsonx.Unmarshal(raw, &res)
	}
	if err != nil {
		return held("it didn't answer: " + err.Error())
	}
	acc := plugin.InterceptResult{Text: in.Text}
	if stop, done := h.take(in.Plugin, in.Text, res, &acc); done {
		return stop
	}
	return h.chain(ctx, in.Intercept, in.Plugin, acc)
}

// chain asks the plugins named after after, in name order, adding their
// changes to acc. acc.Text is the message as it stands when it's not the
// zero value; acc.Plugin who changed it so far.
func (h *uiHub) chain(ctx context.Context, in plugin.Intercept, after string, acc plugin.InterceptResult) plugin.InterceptResult {
	ctx, cancel := context.WithTimeout(ctx, plugin.InterceptBudget)
	defer cancel()
	if acc.Text == "" {
		acc.Text = in.Text
	}
	for _, r := range h.running() {
		if r.name <= after {
			continue
		}
		m, conn := r.live()
		if conn == nil || !m.CanUI(plugin.UIIntercept) {
			continue
		}
		h.mu.Lock()
		skip := h.skipped[r.name] != ""
		h.mu.Unlock()
		if skip {
			continue
		}
		if ctx.Err() != nil {
			break
		}
		each, cancelEach := context.WithTimeout(ctx, plugin.InterceptEach)
		ask := in
		ask.Text = acc.Text
		began := time.Now()
		raw, err := callWithin(each, conn, "ui.intercept", ask)
		cancelEach()
		var res plugin.InterceptResult
		if err == nil {
			err = jsonx.Unmarshal(raw, &res)
		}
		if err != nil {
			// Cut short only by what the others took isn't its fault.
			if !errors.Is(err, context.DeadlineExceeded) || time.Since(began) >= plugin.InterceptEach {
				h.strike(r.name, err)
			}
			continue
		}
		h.mu.Lock()
		delete(h.strikes, r.name)
		h.mu.Unlock()
		if stop, done := h.take(r.name, acc.Text, res, &acc); done {
			return stop
		}
	}
	if acc.Text != in.Text {
		acc.Action = "rewrite"
		return acc
	}
	return plugin.InterceptResult{Action: "allow"}
}

// take adds name's answer res, given the message as text, to acc: a
// rewrite changes it, and a block or a valid ask ends the chain with the
// result to return.
func (h *uiHub) take(name, text string, res plugin.InterceptResult, acc *plugin.InterceptResult) (plugin.InterceptResult, bool) {
	switch res.Action {
	case "block":
		return plugin.InterceptResult{Action: "block", Plugin: name, Reason: plugin.CleanNotice(res.Reason)}, true
	case "rewrite", "ask":
		changed := res.Apply(text)
		if len(changed) > maxText || !utf8.ValidString(changed) {
			return plugin.InterceptResult{}, false
		}
		if res.Action == "ask" {
			if err := plugin.CleanAsk(&res); err != nil {
				return plugin.InterceptResult{}, false
			}
		}
		if changed != text {
			h.changedBy(acc, name, res, changed)
		}
		if res.Action == "ask" {
			acc.Action, acc.ID, acc.Question, acc.Detail, acc.Choices = "ask", res.ID, res.Question, res.Detail, res.Choices
			acc.Input = res.Input
			acc.Plugin = name
			return *acc, true
		}
	}
	return plugin.InterceptResult{}, false
}

// changedBy records that name changed the message to text: its
// replacements and what it appended, kept so a window can change the box
// in place, and who changed it.
func (h *uiHub) changedBy(acc *plugin.InterceptResult, name string, res plugin.InterceptResult, text string) {
	acc.Text = text
	if res.Text == "" {
		acc.Replace = append(acc.Replace, res.Replace...)
		acc.Append += res.Append
	}
	if acc.Plugin == "" {
		acc.Plugin = name
	} else if !slices.Contains(strings.Split(acc.Plugin, ", "), name) {
		acc.Plugin += ", " + name
	}
}

func (h *uiHub) strike(name string, err error) {
	h.mu.Lock()
	h.strikes[name]++
	out := h.strikes[name] >= plugin.InterceptStrikeOut
	if out {
		h.skipped[name] = fmt.Sprintf("didn't answer %d times in a row (%v); skipped until it restarts", plugin.InterceptStrikeOut, err)
	}
	h.mu.Unlock()
	if out {
		h.changed()
	}
}

// gone forgets what a plugin that stopped added, and its strikes.
func (h *uiHub) gone(name string) {
	h.mu.Lock()
	for sess, byPlugin := range h.sections {
		delete(byPlugin, name)
		if len(byPlugin) == 0 {
			delete(h.sections, sess)
		}
	}
	for sess, byPlugin := range h.statuses {
		delete(byPlugin, name)
		if len(byPlugin) == 0 {
			delete(h.statuses, sess)
		}
	}
	for sess, byPlugin := range h.notes {
		delete(byPlugin, name)
		if len(byPlugin) == 0 {
			delete(h.notes, sess)
		}
	}
	delete(h.strikes, name)
	delete(h.skipped, name)
	delete(h.asked, name)
	h.mu.Unlock()
	h.changed()
}

// sessionKey checks a session id a plugin names.
func sessionKey(s string) error {
	if s == "" || len(s) > maxSessionKey || !utf8.ValidString(s) || strings.ContainsFunc(s, func(r rune) bool { return r < 0x20 || r == 0x7f }) {
		return badParams("bad session id")
	}
	return nil
}

// uiParams are the params of a plugin's ui.* calls.
type uiParams struct {
	UI       string           `json:"ui"`
	Session  string           `json:"session"`
	Sections []plugin.Section `json:"sections"`
	Text     string           `json:"text"`
	Tone     string           `json:"tone"`
	Box      *plugin.Box      `json:"box"`
	If       *string          `json:"if"`
	Pick     *plugin.Pick     `json:"pick"`
}

// uiNeeds is the capability each of a plugin's ui.* calls needs.
var uiNeeds = map[string]string{
	"ui.overview.set": plugin.UIOverview,
	"ui.status.set":   plugin.UIOverview,
	"ui.notify":       plugin.UINotify,
	"ui.input.set":    plugin.UIInput,
	"ui.box.note":     plugin.UIInput,
	"ui.pick":         "",
	"ui.send":         plugin.UISend,
	"ui.settings.get": "",
}

// fromPlugin answers a plugin's ui.* calls, each checked against its
// approved manifest.
func (h *uiHub) fromPlugin(p *plugin.Plugin, method string, params jsontext.Value) (any, error) {
	c, ok := uiNeeds[method]
	if !ok {
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	}
	if c != "" && !p.CanUI(c) {
		return nil, plugin.Denied(fmt.Sprintf("%s needs %q in the manifest's ui", method, c))
	}
	var in uiParams
	if len(params) > 0 {
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, badParams(err.Error())
		}
	}
	var err error
	switch method {
	case "ui.overview.set":
		err = h.setOverview(p.Name, &in)
	case "ui.status.set":
		err = h.setStatus(p.Name, &in)
	case "ui.notify":
		err = h.notice(p.Name, &in)
	case "ui.input.set":
		err = h.setInput(p.Name, &in)
	case "ui.box.note":
		err = h.setNote(p.Name, &in)
	case "ui.pick":
		err = h.pick(p.Name, &in)
	case "ui.send":
		err = h.send(p, &in)
	case "ui.settings.get":
		return map[string]any{"values": plugin.SettingValuesOf(&p.Manifest)}, nil
	}
	if err != nil {
		return nil, err
	}
	return map[string]any{}, nil
}

// setOverview replaces a plugin's sections on a session; none removes them.
func (h *uiHub) setOverview(name string, in *uiParams) error {
	if err := sessionKey(in.Session); err != nil {
		return err
	}
	if len(in.Sections) > plugin.MaxSections {
		return badParams(fmt.Sprintf("at most %d sections", plugin.MaxSections))
	}
	var clean []plugin.Section
	seen := map[string]bool{}
	for _, s := range in.Sections {
		c, err := plugin.CleanSection(s)
		if err != nil {
			return badParams(err.Error())
		}
		if seen[c.ID] {
			return badParams("section id " + c.ID + " twice")
		}
		seen[c.ID] = true
		clean = append(clean, c)
	}
	h.mu.Lock()
	err := put(h.sections, in.Session, name, clean, len(clean) == 0)
	h.mu.Unlock()
	if err != nil {
		return err
	}
	h.changed()
	return nil
}

// setStatus replaces a plugin's status on a session; no text removes it.
func (h *uiHub) setStatus(name string, in *uiParams) error {
	if err := sessionKey(in.Session); err != nil {
		return err
	}
	st := plugin.CleanStatus(plugin.Status{Text: in.Text, Tone: in.Tone})
	h.mu.Lock()
	err := put(h.statuses, in.Session, name, st, strings.TrimSpace(st.Text) == "")
	h.mu.Unlock()
	if err != nil {
		return err
	}
	h.changed()
	return nil
}

// put sets or removes what a plugin has on a session, keeping it to
// MaxSessionsTracked sessions. Under h.mu.
func put[V any](m map[string]map[string]V, session, name string, v V, remove bool) error {
	byPlugin := m[session]
	if remove {
		delete(byPlugin, name)
		if len(byPlugin) == 0 {
			delete(m, session)
		}
		return nil
	}
	if _, ok := byPlugin[name]; !ok && countSessions(m, name) >= plugin.MaxSessionsTracked {
		return plugin.Denied(fmt.Sprintf("it has something on %d sessions already", plugin.MaxSessionsTracked))
	}
	if byPlugin == nil {
		byPlugin = map[string]V{}
		m[session] = byPlugin
	}
	byPlugin[name] = v
	return nil
}

// notice shows a short message, at most notifyBurst at once and one a
// second after.
func (h *uiHub) notice(name string, in *uiParams) error {
	text := plugin.CleanNotice(in.Text)
	if strings.TrimSpace(text) == "" {
		return badParams("text is empty")
	}
	now := time.Now()
	h.mu.Lock()
	k := h.notices[name]
	if k == nil {
		k = &bucket{tokens: notifyBurst, at: now}
		h.notices[name] = k
	}
	ok := k.take(now, notifyEvery, notifyBurst)
	h.mu.Unlock()
	if !ok {
		return plugin.Limited("notices, one a second")
	}
	tone := plugin.CleanStatus(plugin.Status{Tone: in.Tone}).Tone
	h.do(plugin.UIDo{Plugin: name, UI: in.UI, Kind: "notify", Session: in.Session, Text: text, Tone: tone})
	return nil
}

// setInput sets what's in a message box.
func (h *uiHub) setInput(name string, in *uiParams) error {
	if in.Session != "" {
		if err := sessionKey(in.Session); err != nil {
			return err
		}
	}
	if len(in.Text) > maxText || !utf8.ValidString(in.Text) {
		return badParams("text is too long")
	}
	if b := in.Box; b != nil {
		if b.Size() > 4*maxText || !utf8.ValidString(b.Text) || b.Cursor < 0 || len(b.Pastes) > maxChips || len(b.Images) > maxChips {
			return badParams("box is too big")
		}
		for _, p := range b.Images {
			if !filepath.IsAbs(p) {
				return badParams("an image is a file's absolute path")
			}
		}
	}
	h.do(plugin.UIDo{Plugin: name, UI: in.UI, Kind: "input.set", Session: in.Session, Text: in.Text, Box: in.Box, If: in.If})
	return nil
}

// pick shows a UI a list to choose from, only in answer to the plugin's
// command or pick there, within commandWait of it.
func (h *uiHub) pick(name string, in *uiParams) error {
	if in.Pick == nil {
		return badParams("no pick")
	}
	h.mu.Lock()
	at, ok := h.asked[name][in.UI]
	h.mu.Unlock()
	if !ok || time.Since(at) > commandWait {
		return plugin.Denied("a pick is shown only in answer to the plugin's command, run in that window")
	}
	p, err := plugin.CleanPick(*in.Pick)
	if err != nil {
		return badParams(err.Error())
	}
	if p.Session != "" {
		if err := sessionKey(p.Session); err != nil {
			return err
		}
	}
	h.do(plugin.UIDo{Plugin: name, UI: in.UI, Kind: "pick", Session: p.Session, Pick: &p})
	return nil
}

// maxChips is how many pastes or images a box set by a plugin may hold.
const maxChips = 64

// setNote replaces a plugin's note on the edge of a session's message box,
// or the Prompt's for no session; no text removes it.
func (h *uiHub) setNote(name string, in *uiParams) error {
	if in.Session != "" {
		if err := sessionKey(in.Session); err != nil {
			return err
		}
	}
	st := plugin.CleanNote(plugin.Status{Text: in.Text, Tone: in.Tone})
	h.mu.Lock()
	err := put(h.notes, in.Session, name, st, strings.TrimSpace(st.Text) == "")
	h.mu.Unlock()
	if err != nil {
		return err
	}
	h.changed()
	return nil
}

// send has a UI send a message as you would: only to a session seen in
// rush's screen, in one of the plugin's workspaces, sendsPerMin a minute.
func (h *uiHub) send(p *plugin.Plugin, in *uiParams) error {
	if err := sessionKey(in.Session); err != nil {
		return err
	}
	if strings.TrimSpace(in.Text) == "" || len(in.Text) > maxText || !utf8.ValidString(in.Text) {
		return badParams("text is empty or too long")
	}
	h.mu.Lock()
	s, seen := h.sessions[in.Session]
	h.mu.Unlock()
	if !seen {
		return plugin.Denied("session " + in.Session + " hasn't been seen in rush's screen")
	}
	dir, err := filepath.EvalSymlinks(s.Cwd)
	if s.Cwd == "" || err != nil || !slices.ContainsFunc(p.WorkspaceDirs(), func(w string) bool { return within(dir, w) }) {
		return plugin.Denied("session " + in.Session + " is outside the plugin's workspaces")
	}
	now := time.Now()
	h.mu.Lock()
	recent := slices.DeleteFunc(h.sends[p.Name], func(t time.Time) bool { return now.Sub(t) > time.Minute })
	if len(recent) >= sendsPerMin {
		h.sends[p.Name] = recent
		h.mu.Unlock()
		return plugin.Limited(fmt.Sprintf("it has sent %d messages this minute", sendsPerMin))
	}
	h.sends[p.Name] = append(recent, now)
	h.mu.Unlock()
	h.do(plugin.UIDo{Plugin: p.Name, UI: in.UI, Kind: "send", Session: in.Session, Text: in.Text})
	return nil
}

// countSessions is how many sessions a plugin has something on in m.
func countSessions[V any](m map[string]map[string]V, name string) int {
	n := 0
	for _, byPlugin := range m {
		if _, ok := byPlugin[name]; ok {
			n++
		}
	}
	return n
}
