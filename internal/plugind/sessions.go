package plugind

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Limits on how much a plugin can start, so a runaway one can't spend your
// usage for you.
const (
	maxLive        = 4  // its sessions running at once
	maxStartsPerHr = 30 // sessions it starts in an hour
	maxText        = 100 << 10
)

// Session is what a plugin is told about a session: what the list shows,
// never what was said.
type Session struct {
	ID string `json:"id"`
	// SessionID is Claude Code's id for the conversation, the one its
	// transcript and hooks go by: how other tools know the session.
	SessionID      string            `json:"sessionId,omitempty"`
	Name           string            `json:"name,omitempty"`
	Cwd            string            `json:"cwd"`
	Repo           string            `json:"repo,omitempty"`   // the checkout cwd is in
	Branch         string            `json:"branch,omitempty"` // what it has checked out
	Worktree       bool              `json:"worktree,omitzero"`
	State          string            `json:"state"`
	Detail         string            `json:"detail,omitempty"`
	Needs          string            `json:"needs,omitempty"`
	Model          string            `json:"model,omitempty"`
	PermissionMode string            `json:"permissionMode,omitempty"`
	CostUSD        float64           `json:"costUsd,omitzero"`
	ContextTokens  int               `json:"contextTokens,omitzero"`
	Queued         int               `json:"queued,omitzero"` // messages waiting for its turn to end
	StartedBy      string            `json:"startedBy,omitempty"`
	Meta           map[string]string `json:"meta,omitempty"`
	StartedAt      time.Time         `json:"startedAt"`
	UpdatedAt      time.Time         `json:"updatedAt"`
}

func sessionOf(i host.Info) Session {
	st := i.State
	if !alive(i.HostPID) {
		st = "stopped"
	}
	g := gitOf(i.Cwd)
	return Session{ID: i.ID, SessionID: i.SessionID, Name: i.Name, Cwd: i.Cwd, Repo: g.repo, Branch: g.branch, Worktree: g.worktree,
		State: st, Detail: i.Detail, Needs: i.Needs, Model: i.Model, PermissionMode: i.PermissionMode,
		CostUSD: i.CostUSD, ContextTokens: i.ContextTokens, Queued: len(i.Queue), StartedBy: i.StartedBy, Meta: i.Meta,
		StartedAt: i.StartedAt, UpdatedAt: i.UpdatedAt}
}

func alive(pid int) bool {
	if pid <= 0 {
		return false
	}
	err := syscall.Kill(pid, 0)
	return err == nil || errors.Is(err, syscall.EPERM)
}

// fromPlugin answers the plugin's own calls, each checked against what it
// was approved for. Sending, reading and control reach only sessions it
// started: another session may run with permissions it was never given.
func (r *runner) fromPlugin(ctx context.Context, method string, params jsontext.Value) (any, error) {
	r.mu.Lock()
	p := r.p
	r.mu.Unlock()
	need := func(c string) error {
		if !p.Can(c) {
			return plugin.Denied(fmt.Sprintf("%s needs the %q capability", method, c))
		}
		return nil
	}
	if strings.HasPrefix(method, "ui.") {
		if r.b == nil || r.b.ui == nil {
			return nil, errors.New("rush's screen isn't reachable")
		}
		return r.b.ui.fromPlugin(&p, method, params)
	}
	var in struct {
		ID             string            `json:"id"`
		Text           string            `json:"text"`
		Now            bool              `json:"now"`
		Cwd            string            `json:"cwd"`
		Prompt         string            `json:"prompt"`
		Name           string            `json:"name"`
		Model          string            `json:"model"`
		Effort         string            `json:"effort"`
		PermissionMode string            `json:"permissionMode"`
		Worktree       *worktreeReq      `json:"worktree"`
		Meta           map[string]string `json:"meta"`
		Message        string            `json:"message"`
		Args           []string          `json:"args"`
		Stdin          string            `json:"stdin"`
		Index          int               `json:"index"`
		Was            string            `json:"was"`
	}
	if len(params) > 0 {
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: err.Error()}
		}
	}
	switch method {
	case "log":
		msg := in.Message
		if len(msg) > 1024 {
			msg = msg[:1024] + "…"
		}
		r.log.Printf("says: %s", strings.ReplaceAll(msg, "\n", " "))
		return map[string]any{}, nil

	case "sessions.list":
		if err := need(plugin.CapList); err != nil {
			return nil, err
		}
		out := []Session{}
		for _, i := range host.List() {
			out = append(out, sessionOf(i))
		}
		return out, nil

	case "sessions.watch":
		if err := need(plugin.CapList); err != nil {
			return nil, err
		}
		return r.watchList()

	case "sessions.unwatch":
		r.mu.Lock()
		if r.unwatch != nil {
			r.unwatch()
			r.unwatch = nil
		}
		r.mu.Unlock()
		return map[string]any{}, nil

	case "sessions.start":
		if err := need(plugin.CapStart); err != nil {
			return nil, err
		}
		return r.start(p, startReq{cwd: in.Cwd, prompt: in.Prompt, name: in.Name, model: in.Model, effort: in.Effort,
			mode: in.PermissionMode, worktree: in.Worktree, meta: in.Meta})

	case "sessions.queue":
		if err := need(plugin.CapQueue); err != nil {
			return nil, err
		}
		if err := r.queueable(p, in.ID); err != nil {
			return nil, err
		}
		if strings.TrimSpace(in.Text) == "" || len(in.Text) > maxText {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "text is empty or too long"}
		}
		// Said to be from the plugin, so neither you nor Claude takes it
		// for yours.
		text := "[from the rush plugin " + p.Name + "]\n" + in.Text
		return withHost(in.ID, func(c *host.Client) error { return c.Send(text) })

	case "sessions.queued.send", "sessions.queued.remove":
		if err := need(plugin.CapQueued); err != nil {
			return nil, err
		}
		if _, err := r.inWorkspaces(p, in.ID); err != nil {
			return nil, err
		}
		if len(in.Was) > maxText {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "was is too long"}
		}
		if in.Was != "" && !p.Bundled {
			// Naming a message by its text would let a plugin test guesses
			// at what's queued, which it's never to see.
			return nil, plugin.Denied("only rush's own plugins name a queued message by its text")
		}
		return queued(in.ID, in.Index, in.Was, method == "sessions.queued.send")

	case "sidebar.set":
		return r.setSidebar(p, params)

	case "exec":
		return r.exec(ctx, p, in.Name, in.Args, in.Stdin, in.Cwd)

	case "sessions.send":
		if err := need(plugin.CapSend); err != nil {
			return nil, err
		}
		if err := r.own(p, in.ID); err != nil {
			return nil, err
		}
		if strings.TrimSpace(in.Text) == "" || len(in.Text) > maxText {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "text is empty or too long"}
		}
		return withHost(in.ID, func(c *host.Client) error {
			if in.Now {
				return c.SendNow(in.Text)
			}
			return c.Send(in.Text)
		})

	case "sessions.interrupt", "sessions.stop":
		if err := need(plugin.CapControl); err != nil {
			return nil, err
		}
		if err := r.own(p, in.ID); err != nil {
			return nil, err
		}
		return withHost(in.ID, func(c *host.Client) error {
			if method == "sessions.stop" {
				return c.Stop()
			}
			return c.Interrupt()
		})

	case "sessions.subscribe":
		if err := need(plugin.CapRead); err != nil {
			return nil, err
		}
		if err := r.own(p, in.ID); err != nil {
			return nil, err
		}
		return map[string]any{}, r.subscribe(in.ID)

	case "sessions.unsubscribe":
		r.mu.Lock()
		if s := r.subs[in.ID]; s != nil {
			s.cancel()
			delete(r.subs, in.ID)
		}
		r.mu.Unlock()
		return map[string]any{}, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// setSidebar keeps how the plugin arranges rush's agent list, once it is
// checked and cleaned, where the UI reads it and the plugin can't write.
func (r *runner) setSidebar(p plugin.Plugin, params jsontext.Value) (any, error) {
	if !p.Sidebar {
		return nil, plugin.Denied(`sidebar.set needs "sidebar" in the manifest`)
	}
	s, err := plugin.ParseSidebar(p.Name, params)
	if err != nil {
		return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: err.Error()}
	}
	if err := plugin.SaveSidebar(s); err != nil {
		return nil, err
	}
	return map[string]any{}, nil
}

// idRE is a session id as rush makes them. An id becomes a path, so
// nothing else gets near the filesystem.
var idRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`)

// own checks the session is one the plugin started.
func (r *runner) own(p plugin.Plugin, id string) error {
	if !idRE.MatchString(id) {
		return &plugin.Error{Code: plugin.CodeInvalidParams, Message: "bad session id"}
	}
	info, err := host.ReadInfo(id)
	if err != nil {
		return &plugin.Error{Code: plugin.CodeInvalidParams, Message: "no session " + id}
	}
	if info.StartedBy != p.Name {
		return plugin.Denied("session " + id + " was not started by " + p.Name)
	}
	return nil
}

// safeModes are the permission modes that ask you before anything your
// settings don't already allow: the only ones a plugin may start a session
// in, or queue to one in.
var safeModes = []string{"default", "acceptEdits", "plan"}

// queueable checks a plugin may queue to the session: one running in its
// workspaces, in a mode that still asks you.
func (r *runner) queueable(p plugin.Plugin, id string) error {
	info, err := r.inWorkspaces(p, id)
	if err != nil || info.StartedBy == p.Name {
		return err
	}
	if m := info.PermissionMode; m != "" && !slices.Contains(safeModes, m) {
		return plugin.Denied("session " + id + " runs in " + m + " mode, which doesn't ask you first")
	}
	return nil
}

// inWorkspaces checks the session is one the plugin started, or runs in
// its workspaces.
func (r *runner) inWorkspaces(p plugin.Plugin, id string) (host.Info, error) {
	if !idRE.MatchString(id) {
		return host.Info{}, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "bad session id"}
	}
	info, err := host.ReadInfo(id)
	if err != nil {
		return host.Info{}, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "no session " + id}
	}
	if info.StartedBy == p.Name {
		return info, nil
	}
	dir, err := filepath.EvalSymlinks(info.Cwd)
	if err != nil || !slices.ContainsFunc(p.WorkspaceDirs(), func(w string) bool { return within(dir, w) }) {
		return host.Info{}, plugin.Denied("session " + id + " is outside the plugin's workspaces")
	}
	return info, nil
}

// queued sends now, or drops, the message at index in a session's queue,
// and waits until the session says it's gone. was, when given, names it
// by its text, so it's still the one meant if the queue moved.
func queued(id string, index int, was string, send bool) (any, error) {
	c, err := host.Dial(id)
	if err != nil {
		return nil, fmt.Errorf("session %s is not running", id)
	}
	defer c.Close()
	var queue []string
	timeout := time.After(5 * time.Second)
	next := func(want func(any) (bool, error)) error {
		for {
			select {
			case l, ok := <-c.Lines:
				if !ok {
					return fmt.Errorf("session %s went away", id)
				}
				if ev, err := host.Decode(l); err == nil {
					if done, err := want(ev); done || err != nil {
						return err
					}
				}
			case <-timeout:
				return fmt.Errorf("session %s didn't answer", id)
			}
		}
	}
	if err := next(func(ev any) (bool, error) {
		e, ok := ev.(host.InfoEvent)
		if ok {
			queue = e.Info.Queue
		}
		return ok, nil
	}); err != nil {
		return nil, err
	}
	if was == "" {
		if index < 0 || index >= len(queue) {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: fmt.Sprintf("no queued message %d", index)}
		}
		was = queue[index]
	}
	if !slices.Contains(queue, was) {
		return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: "no such message in the queue"}
	}
	count := func(q []string) int {
		return len(slices.DeleteFunc(slices.Clone(q), func(s string) bool { return s != was }))
	}
	n := count(queue)
	if send {
		err = c.SendQueued(index, was)
	} else {
		err = c.RemoveQueued(index, was)
	}
	if err != nil {
		return nil, err
	}
	return map[string]any{}, next(func(ev any) (bool, error) {
		switch e := ev.(type) {
		case host.ErrorEvent:
			return true, errors.New(e.Error)
		case host.InfoEvent:
			return count(e.Info.Queue) < n, nil
		}
		return false, nil
	})
}

func withHost(id string, do func(*host.Client) error) (any, error) {
	c, err := host.Dial(id)
	if err != nil {
		return nil, fmt.Errorf("session %s is not running", id)
	}
	defer c.Close()
	return map[string]any{}, do(c)
}

var modelRE = regexp.MustCompile(`^[A-Za-z0-9._\[\]-]{1,64}$`)

// refRE is a branch or commit a plugin may name: nothing git could take
// for an option.
var refRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$`)

var metaKeyRE = regexp.MustCompile(`^[A-Za-z][A-Za-z0-9_.-]{0,63}$`)

// worktreeReq asks for the session to run in a new git worktree of the
// checkout cwd is in.
type worktreeReq struct {
	Name   string `json:"name"`   // its folder, under .claude/worktrees
	Branch string `json:"branch"` // the new branch; worktree-<name> if ""
	Base   string `json:"base"`   // what the branch starts from; HEAD if ""
}

type startReq struct {
	cwd, prompt, name, model, effort, mode string
	worktree                               *worktreeReq
	meta                                   map[string]string
}

// start starts a session for the plugin: in one of its workspaces, on your
// account and your settings, but never in a mode that skips asking you.
func (r *runner) start(p plugin.Plugin, q startReq) (any, error) {
	cwd, prompt, name, model, effort, mode := q.cwd, q.prompt, q.name, q.model, q.effort, q.mode
	bad := func(msg string) error { return &plugin.Error{Code: plugin.CodeInvalidParams, Message: msg} }
	if !filepath.IsAbs(cwd) {
		return nil, bad("cwd must be an absolute path")
	}
	dir, err := filepath.EvalSymlinks(cwd)
	if err != nil {
		return nil, bad("cwd does not exist")
	}
	if fi, err := os.Stat(dir); err != nil || !fi.IsDir() {
		return nil, bad("cwd is not a folder")
	}
	if !slices.ContainsFunc(p.WorkspaceDirs(), func(w string) bool { return within(dir, w) }) {
		return nil, plugin.Denied(cwd + " is outside the plugin's workspaces")
	}
	if mode == "" {
		mode = "default"
	}
	if !slices.Contains(safeModes, mode) {
		return nil, plugin.Denied("permission mode " + mode + ": use default, acceptEdits or plan")
	}
	if len(q.meta) > 16 {
		return nil, bad("meta: at most 16 keys")
	}
	for k, v := range q.meta {
		if !metaKeyRE.MatchString(k) || len(v) > 1024 {
			return nil, bad("meta: keys are letters, digits, _ . - (64 at most); values 1 KB at most")
		}
	}
	var wtName, wtBranch string
	if wt := q.worktree; wt != nil {
		wtName = worktreeName(or(wt.Name, or(wt.Branch, or(name, firstWords(prompt, 6)))))
		wtBranch = or(wt.Branch, "worktree-"+wtName)
		if !refRE.MatchString(wtBranch) || strings.Contains(wtBranch, "..") || (wt.Base != "" && !refRE.MatchString(wt.Base)) {
			return nil, bad("worktree: bad branch or base")
		}
		// The worktree goes in the checkout's own folder, which must be
		// one of the plugin's too.
		root := actions.RepoRoot(dir)
		if root == "" {
			return nil, bad(cwd + " isn't in a git repository")
		}
		if root, _ = filepath.EvalSymlinks(root); !slices.ContainsFunc(p.WorkspaceDirs(), func(w string) bool { return within(root, w) }) {
			return nil, plugin.Denied("the checkout " + root + " is outside the plugin's workspaces")
		}
	}
	if model != "" && !modelRE.MatchString(model) {
		return nil, bad("bad model name")
	}
	if effort != "" && !slices.Contains([]string{"low", "medium", "high", "xhigh", "max"}, effort) {
		return nil, bad("effort: use low, medium, high, xhigh or max")
	}
	if len(prompt) > maxText {
		return nil, bad("prompt is too long")
	}

	r.mu.Lock()
	now := time.Now()
	r.starts = slices.DeleteFunc(r.starts, func(t time.Time) bool { return now.Sub(t) > time.Hour })
	if len(r.starts) >= maxStartsPerHr {
		r.mu.Unlock()
		return nil, plugin.Denied(fmt.Sprintf("it has started %d sessions this hour", maxStartsPerHr))
	}
	r.starts = append(r.starts, now)
	r.mu.Unlock()
	live := 0
	for _, i := range host.List() {
		if i.StartedBy == p.Name && alive(i.HostPID) {
			live++
		}
	}
	if live >= maxLive {
		return nil, plugin.Denied(fmt.Sprintf("it already has %d sessions running", live))
	}

	if q.worktree != nil {
		wt, err := actions.NewWorktreeOn(dir, wtName, wtBranch, q.worktree.Base)
		if err != nil {
			return nil, err
		}
		dir = wt
	}
	if name = strings.TrimSpace(name); name == "" {
		name = firstWords(prompt, 6)
	}
	name = p.Name + ": " + name
	st := state.Load()
	d := st.Config.Dispatch
	cfg := host.Config{
		Account: st.Config.ActiveAccount().Profile(), Cwd: dir, Prompt: prompt, Name: name,
		Model: or(model, d.Model), Effort: or(effort, d.Effort), PermissionMode: mode,
		LimitMode: d.OnLimit, Lean: d.Lean, IdleStop: host.Duration(d.Rest()), StartedBy: p.Name, Meta: q.meta,
	}
	started, err := host.Spawn(cfg)
	if err != nil {
		return nil, err
	}
	r.log.Printf("started session %s in %s", started.ID, dir)
	return map[string]any{"id": started.ID, "cwd": dir}, nil
}

var worktreeUnsafe = regexp.MustCompile(`[^a-z0-9]+`)

// worktreeName is a folder name made from s, as rush's forks name theirs.
func worktreeName(s string) string {
	s = strings.Trim(worktreeUnsafe.ReplaceAllString(strings.ToLower(s), "-"), "-")
	if len(s) > 40 {
		s = strings.TrimRight(s[:40], "-")
	}
	return or(s, "session")
}

func within(dir, root string) bool {
	return dir == root || strings.HasPrefix(dir, strings.TrimSuffix(root, "/")+"/")
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

func firstWords(s string, n int) string {
	w := strings.Fields(s)
	if len(w) > n {
		w = w[:n]
	}
	if len(w) == 0 {
		return "a new session"
	}
	return strings.Join(w, " ")
}

// subscribe follows a session and passes its events on as session.event
// notifications: its state, Claude's words, the tools it uses (by what they
// do, not what they return) and the end of each turn.
func (r *runner) subscribe(id string) error {
	c, err := host.Dial(id)
	if err != nil {
		return fmt.Errorf("session %s is not running", id)
	}
	ctx, cancel := context.WithCancel(context.Background())
	me := &sub{cancel: cancel}
	r.mu.Lock()
	if old := r.subs[id]; old != nil {
		old.cancel()
	}
	r.subs[id] = me
	conn := r.conn
	r.mu.Unlock()
	if conn == nil {
		cancel()
		c.Close()
		return errors.New("not ready")
	}
	go func() {
		<-ctx.Done()
		c.Close()
	}()
	go func() {
		defer cancel()
		emit := func(ev map[string]any) {
			ev["session"] = id
			_ = conn.Notify("session.event", ev)
		}
		// The host replays what it has before its info; only what happens
		// from now on is news.
		live := false
		var last Session
		var d host.Decoder
		var kind agent.Kind
		for line := range c.Lines {
			evs, err := d.Decode(line)
			if err != nil {
				continue
			}
			for _, ev := range evs {
				switch ev := ev.(type) {
				case host.InfoEvent:
					live, kind = true, agent.Kind(ev.Info.Kind)
					s := sessionOf(ev.Info)
					if s.State != last.State || s.Detail != last.Detail || s.Needs != last.Needs {
						emit(map[string]any{"type": "info", "state": s.State, "detail": s.Detail, "needs": s.Needs, "costUsd": s.CostUSD})
					}
					last = s
				case host.Sent:
					if live {
						emit(map[string]any{"type": "sent", "text": ev.Text})
					}
				case event.Message:
					if live && ev.Role == "assistant" && ev.Parent == "" {
						said(&ev, kind, emit)
					}
				case event.TurnEnd:
					if live {
						failed, text := ev.Err != "" || ev.Reason != "done" && ev.Reason != "interrupted", ev.Text
						if ev.Err != "" {
							text = ev.Err
						}
						emit(map[string]any{"type": "result", "text": text, "isError": failed,
							"costUsd": ev.Cost, "turns": ev.Turns})
					}
				}
			}
		}
		if ctx.Err() == nil {
			emit(map[string]any{"type": "closed"})
		}
		r.mu.Lock()
		if r.subs[id] == me {
			delete(r.subs, id)
		}
		r.mu.Unlock()
	}()
	return nil
}

// said tells of what the main agent said: its words, and each tool call
// in a few words, as agent k puts it.
func said(m *event.Message, k agent.Kind, emit func(map[string]any)) {
	for _, p := range m.Parts {
		switch {
		case p.Kind == event.Text && strings.TrimSpace(p.Text) != "":
			emit(map[string]any{"type": "text", "text": p.Text})
		case p.Kind == event.ToolCall && p.Call != nil:
			emit(map[string]any{"type": "tool", "name": p.Call.Name, "doing": agent.Doing(k, p.Call)})
		}
	}
}

// sub is one followed session.
type sub struct{ cancel context.CancelFunc }
