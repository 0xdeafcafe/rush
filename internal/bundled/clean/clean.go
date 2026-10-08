// Package clean is rush's bundled clean-up plugins: go-clean, js-clean and
// git-clean. Each is off until you turn it on, and runs only with its tools
// installed. Its one command lists what that ecosystem keeps on your disk
// (caches, stores, dependencies in agents' worktrees, merged branches) with
// how much each takes, and what of it is running (gopls, node), with how
// much memory; enter on a row cleans it up or ends it.
//
// Nothing goes by itself: sizes are measured in the background, and only
// what you pick is cleaned.
package clean

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/efficiency"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/proc"
)

func init() {
	for _, k := range []*kit{goKit, jsKit, gitKit} {
		plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: k.manifest(), Run: k.run})
	}
}

// A kit is one ecosystem's clean-up.
type kit struct {
	name, about string
	needs       []string
	// procs are the process names it owns, as the kernel has them.
	procs []string
	// things finds what it keeps on disk, by the folders sessions were
	// seen in. It may run programs; sizes are measured after.
	things func(ctx context.Context, seen []session) []thing
}

// A thing is one row of what a kit keeps.
type thing struct {
	ID, Title string
	Path      string // measured for its size, when set
	Meta      string // shown instead of a size, when set
	Does      string // what enter does, under its title
	// In is a worktree it's in: it isn't cleaned while a session works
	// there.
	In string
	// Clean cleans it up, and says what it did beyond what it freed.
	Clean func(ctx context.Context) (string, error)
	size  int64
}

// session is what a kit knows of a session seen in rush: where it works,
// and whether it's working now.
type session struct {
	ID, Cwd, State string
}

func (k *kit) manifest() plugin.Manifest {
	return plugin.Manifest{
		Name:        k.name,
		Description: k.about,
		Command:     []string{"rush"},
		UI:          []string{plugin.UIEvents, plugin.UINotify},
		Commands:    []plugin.CommandSpec{{Name: "clean", Description: "what " + k.name + " finds on disk and running, and cleaning it up"}},
		Requires:    plugin.Requires{OS: []string{"darwin", "linux"}, Bin: k.needs}, // proc reads processes only there
		MemoryMB:    128,
	}
}

// measureEvery is how often sizes are measured again, while it runs.
const measureEvery = 15 * time.Minute

type app struct {
	k     *kit
	conn  *plugin.Conn
	ready chan struct{}

	mu        sync.Mutex
	seen      map[string]session
	things    []thing
	measured  time.Time
	measuring bool
	busy      map[string]bool // things being cleaned
	ctx       context.Context
}

func (k *kit) run(rw io.ReadWriteCloser) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	a := &app{k: k, ready: make(chan struct{}), seen: map[string]session{}, busy: map[string]bool{}, ctx: ctx}
	a.conn = plugin.NewConn(rw, a.handle)
	close(a.ready)
	go a.loop(ctx)
	<-a.conn.Done()
	return nil
}

func (a *app) handle(_ context.Context, method string, params jsontext.Value) (any, error) {
	<-a.ready
	switch method {
	case "initialize", "ui.settings":
		return map[string]any{}, nil
	case "tools.list":
		return map[string]any{"tools": []any{}}, nil
	case "ui.event":
		var ev plugin.UIEvent
		if jsonx.Unmarshal(params, &ev) == nil {
			a.event(ev)
		}
		return nil, nil
	case "ui.command":
		var in struct {
			Command string `json:"command"`
			UI      string `json:"ui"`
		}
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		go a.show(in.UI)
		return map[string]any{}, nil
	case "ui.picked":
		var in plugin.Picked
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		if in.Action == "enter" {
			go a.clean(in.UI, in.Item)
		}
		return map[string]any{}, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// event keeps where sessions work and whether they're working.
func (a *app) event(ev plugin.UIEvent) {
	if ev.Session == nil || ev.Session.Cwd == "" {
		return
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	s := session{ID: ev.Session.ID, Cwd: ev.Session.Cwd, State: ev.Session.State}
	switch ev.Kind {
	case plugin.EvTurnStarted:
		s.State = "working"
	case plugin.EvTurnEnded, plugin.EvSessionStopped:
		if s.State == "working" {
			s.State = "idle"
		}
	}
	was, ok := a.seen[s.ID]
	a.seen[s.ID] = s
	if (!ok || was.Cwd != s.Cwd) && !a.measuring {
		// A folder came in: what's in it is worth measuring soon.
		a.measured = time.Time{}
	}
}

func (a *app) sessions() []session {
	a.mu.Lock()
	defer a.mu.Unlock()
	out := make([]session, 0, len(a.seen))
	for _, s := range a.seen {
		out = append(out, s)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// loop measures what it keeps now and then, and soon after sessions are
// first seen: it waits a tick first, for the ones rush tells it of as it
// connects.
func (a *app) loop(ctx context.Context) {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		a.mu.Lock()
		due := time.Since(a.measured) > measureEvery
		a.mu.Unlock()
		if due {
			a.measure(ctx)
		}
	}
}

func (a *app) measure(ctx context.Context) {
	a.mu.Lock()
	if a.measuring {
		a.mu.Unlock()
		return
	}
	a.measuring = true
	a.mu.Unlock()
	things := a.k.things(ctx, a.sessions())
	for i := range things {
		if things[i].Path != "" {
			things[i].size = fleet.DiskUsage([]fleet.TempDir{{Path: things[i].Path}})
		}
	}
	a.mu.Lock()
	a.things, a.measured, a.measuring = things, time.Now(), false
	a.mu.Unlock()
}

// show opens the list in the window that asked.
func (a *app) show(ui string) {
	a.mu.Lock()
	things, measured := slices.Clone(a.things), a.measured
	a.mu.Unlock()
	p := a.pick(things, measured, a.k.running())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = a.conn.Call(ctx, "ui.pick", map[string]any{"ui": ui, "pick": p}, nil)
}

func (a *app) pick(things []thing, measured time.Time, running []running) plugin.Pick {
	p := plugin.Pick{
		ID:      "clean",
		Title:   a.k.name,
		Tabs:    []string{"On disk"},
		Actions: []plugin.PickAction{{Key: "enter", Name: "clean up", Stay: true}},
		Empty:   []string{"Nothing found yet."},
	}
	switch {
	case measured.IsZero():
		p.About, p.Empty[0] = "Measuring what's on disk; look again in a moment.", "Measuring…"
	default:
		p.About = "Measured " + ago(measured) + ". Enter on a row cleans it up."
	}
	var total int64
	for _, t := range things {
		meta := t.Meta
		if meta == "" {
			meta = efficiency.Bytes(t.size)
			total += t.size
		}
		lines := []string{t.Title, "enter " + t.Does}
		if t.Path != "" {
			lines = slices.Insert(lines, 1, t.Path)
		}
		p.Items = append(p.Items, plugin.PickItem{ID: t.ID, Text: strings.Join(lines, "\n"), Meta: meta})
	}
	if total > 0 {
		p.Tabs[0] = "On disk · " + efficiency.Bytes(total)
	}
	if len(a.k.procs) > 0 {
		p.Tabs = append(p.Tabs, "Running")
		p.Empty = append(p.Empty, "None of "+strings.Join(a.k.procs, ", ")+" is running.")
		var mem uint64
		for _, r := range running {
			mem += r.Footprint
			text := fmt.Sprintf("%s · pid %d · up %s\n%s\nenter ends it", r.Comm, r.PID, ago(r.Start), r.Cmd)
			p.Items = append(p.Items, plugin.PickItem{ID: r.id(), Tab: 1, Text: text, Meta: efficiency.Bytes(int64(r.Footprint))})
		}
		if mem > 0 {
			p.Tabs[1] = "Running · " + efficiency.Bytes(int64(mem))
		}
	}
	return p
}

// clean cleans up the thing, or ends the process, picked.
func (a *app) clean(ui, id string) {
	if strings.HasPrefix(id, "pid:") {
		a.notify(ui, a.end(id))
		return
	}
	a.mu.Lock()
	i := slices.IndexFunc(a.things, func(t thing) bool { return t.ID == id })
	if i < 0 || a.busy[id] {
		a.mu.Unlock()
		return
	}
	t := a.things[i]
	if t.In != "" && a.workingIn(t.In) {
		a.mu.Unlock()
		a.notify(ui, t.Title+": an agent is working there now, so it's left alone")
		return
	}
	a.busy[id] = true
	a.mu.Unlock()
	a.notify(ui, "cleaning up "+t.Title+"…")
	ctx, cancel := context.WithTimeout(a.ctx, 10*time.Minute)
	said, err := t.Clean(ctx)
	cancel()
	var freed int64
	if t.Path != "" {
		freed = t.size - fleet.DiskUsage([]fleet.TempDir{{Path: t.Path}})
	}
	a.mu.Lock()
	delete(a.busy, id)
	a.measured = time.Time{} // measure again, soon
	a.mu.Unlock()
	switch {
	case err != nil:
		a.notify(ui, t.Title+": "+err.Error())
	case freed > 0:
		a.notify(ui, strings.TrimSpace(t.Title+": freed "+efficiency.Bytes(freed)+". "+said))
	default:
		a.notify(ui, strings.TrimSpace(t.Title+": done. "+said))
	}
}

// workingIn says whether a session seen is working in dir. Under a.mu.
func (a *app) workingIn(dir string) bool {
	for _, s := range a.seen {
		if s.State == "working" && (s.Cwd == dir || strings.HasPrefix(s.Cwd, dir+"/")) {
			return true
		}
	}
	return false
}

func (a *app) notify(ui, text string) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = a.conn.Call(ctx, "ui.notify", map[string]any{"ui": ui, "text": text}, nil)
}

// running is a process a kit owns.
type running struct {
	*proc.Proc
	Cmd string
}

func (r running) id() string { return fmt.Sprintf("pid:%d:%d", r.PID, r.Start.UnixNano()) }

// running lists the kit's processes, biggest first.
func (k *kit) running() []running {
	if len(k.procs) == 0 {
		return nil
	}
	tab := proc.Snapshot(nil)
	var out []running
	var pids []int
	for _, p := range tab.Procs {
		if slices.Contains(k.procs, p.Comm) && p.PID != os.Getpid() {
			pids = append(pids, p.PID)
		}
	}
	tab.Fill(nil, pids)
	for _, pid := range pids {
		p, cmd := tab.Procs[pid], proc.CommandLine(pid)
		if cmd == "" && p.Footprint == 0 {
			continue // gone, or not ours to look at
		}
		out = append(out, running{Proc: p, Cmd: cmd})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Footprint > out[j].Footprint })
	return out
}

// end ends the process id names, if it's still the one listed.
func (a *app) end(id string) string {
	parts := strings.Split(id, ":")
	if len(parts) != 3 {
		return "no such process"
	}
	pid, _ := strconv.Atoi(parts[1])
	start, _ := strconv.ParseInt(parts[2], 10, 64)
	for _, r := range a.k.running() {
		if r.PID == pid && r.Start.UnixNano() == start {
			if err := proc.Kill(pid, syscall.SIGTERM); err != nil {
				return fmt.Sprintf("%s (pid %d): %v", r.Comm, pid, err)
			}
			return fmt.Sprintf("ended %s (pid %d), which held %s", r.Comm, pid, efficiency.Bytes(int64(r.Footprint)))
		}
	}
	return fmt.Sprintf("pid %d has already gone", pid)
}

func ago(t time.Time) string {
	d := time.Since(t)
	switch {
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return strconv.Itoa(int(d.Minutes())) + "m ago"
	case d < 48*time.Hour:
		return strconv.Itoa(int(d.Hours())) + "h ago"
	}
	return strconv.Itoa(int(d.Hours()/24)) + "d ago"
}

// command runs a tool it needs, found as rush finds agents, and gives
// back what it printed.
func command(ctx context.Context, dir, name string, args ...string) (string, error) {
	bin, ok := agent.Find(name)
	if !ok {
		return "", errors.New(name + " isn't installed")
	}
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Dir = dir
	// A look must not block an agent's own git.
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0")
	out, err := cmd.Output()
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) && len(ee.Stderr) > 0 {
			return "", fmt.Errorf("%s: %s", name, strings.TrimSpace(lastLine(string(ee.Stderr))))
		}
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

func lastLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.LastIndexByte(s, '\n'); i >= 0 {
		return s[i+1:]
	}
	return s
}

// ran is a Clean that runs a command.
func ran(dir, name string, args ...string) func(context.Context) (string, error) {
	return func(ctx context.Context) (string, error) {
		_, err := command(ctx, dir, name, args...)
		return "", err
	}
}

// removed is a Clean that deletes a folder.
func removed(path string) func(context.Context) (string, error) {
	return func(context.Context) (string, error) { return "", os.RemoveAll(path) }
}

// isDir says whether path is a folder.
func isDir(path string) bool {
	st, err := os.Stat(path)
	return err == nil && st.IsDir()
}

// cacheDir is a folder under the user's cache folder.
func cacheDir(name string) string {
	d, err := os.UserCacheDir()
	if err != nil {
		return ""
	}
	return filepath.Join(d, name)
}

// worktrees are the linked worktrees of every repo sessions were seen in,
// each with whether a session is working in it now.
func worktrees(seen []session) (wts []fleet.Worktree, busy map[string]bool) {
	agents := make([]*fleet.Agent, 0, len(seen))
	state := map[string]string{}
	for _, s := range seen {
		a := &fleet.Agent{Key: s.ID}
		a.Cwd = s.Cwd
		agents = append(agents, a)
		state[s.ID] = s.State
	}
	wts = fleet.FindWorktrees(agents)
	busy = map[string]bool{}
	for i := range wts {
		for _, k := range wts[i].Agents {
			if state[k] == "working" {
				busy[wts[i].Path] = true
			}
		}
	}
	return wts, busy
}

// repos are the main checkouts sessions were seen in.
func repos(ctx context.Context, seen []session) []string {
	set, cwds := map[string]bool{}, map[string]bool{}
	for _, s := range seen {
		if cwds[s.Cwd] {
			continue
		}
		cwds[s.Cwd] = true
		common, err := command(ctx, s.Cwd, "git", "rev-parse", "--path-format=absolute", "--git-common-dir")
		if err != nil {
			continue
		}
		set[filepath.Dir(common)] = true
	}
	out := make([]string, 0, len(set))
	for r := range set {
		out = append(out, r)
	}
	sort.Strings(out)
	return out
}
