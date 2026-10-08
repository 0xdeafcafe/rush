// Package plugind is the plugin broker: the one long-lived `rush plugind`
// process that runs every approved plugin, sandboxed, and stands between it
// and everything else. Session hosts send it the MCP messages Claude Code
// addresses to a plugin's tools; plugins call it to list, start, follow and
// message sessions, and it checks each call against what the plugin was
// approved for before doing it.
//
// It restarts a plugin that exits (waiting longer each time), ends one that
// grows past its memory limit, stops one whose files change, and exits
// itself once no plugin is approved.
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
	"os/exec"
	"os/signal"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// Version is the plugin protocol this broker speaks, sent in initialize.
const Version = 1

// broker runs the plugins.
type broker struct {
	log *log.Logger

	mu      sync.Mutex
	plugins map[string]*runner
	ui      *uiHub
	quit    chan struct{}
	once    sync.Once
}

// Run is `rush plugind`. It returns at once if another broker holds the
// lock.
func Run() error {
	if err := os.MkdirAll(plugin.Root(), 0o700); err != nil {
		return err
	}
	lock, err := os.OpenFile(plugin.BrokerLock(), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		return nil // another broker is running
	}
	sock := plugin.BrokerSock()
	_ = os.Remove(sock)
	// Only you can connect: the folder is 0700 and the socket 0600.
	old := syscall.Umask(0o177)
	ln, err := net.Listen("unix", sock)
	syscall.Umask(old)
	if err != nil {
		return err
	}
	defer os.Remove(sock)

	b := &broker{log: log.New(os.Stderr, "plugind ", log.LstdFlags), plugins: map[string]*runner{}, quit: make(chan struct{})}
	b.ui = newUIHub(b)
	b.log.Printf("started, pid %d", os.Getpid())
	b.reload()
	go b.watch()
	sigs := make(chan os.Signal, 1)
	signal.Notify(sigs, syscall.SIGTERM, syscall.SIGINT, syscall.SIGHUP)
	go func() {
		for s := range sigs {
			if s == syscall.SIGHUP {
				b.reload()
				continue
			}
			b.stop()
		}
	}()
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			b.accept(c)
		}
	}()
	<-b.quit
	_ = ln.Close()
	b.mu.Lock()
	rs := make([]*runner, 0, len(b.plugins))
	for _, r := range b.plugins {
		rs = append(rs, r)
	}
	b.mu.Unlock()
	var wg sync.WaitGroup
	for _, r := range rs {
		wg.Add(1)
		go func() { defer wg.Done(); r.shutdown() }()
	}
	wg.Wait()
	b.log.Printf("stopped")
	return nil
}

// accept answers one connection on broker.sock. A UI's calls need its
// connection, to send it what changes.
func (b *broker) accept(c io.ReadWriteCloser) *plugin.Conn {
	var conn *plugin.Conn
	ready := make(chan struct{})
	conn = plugin.NewConn(c, func(ctx context.Context, method string, params jsontext.Value) (any, error) {
		<-ready
		if strings.HasPrefix(method, "ui.") {
			return b.ui.fromUI(ctx, conn, method, params)
		}
		return b.fromRush(ctx, method, params)
	})
	close(ready)
	return conn
}

func (b *broker) stop() { b.once.Do(func() { close(b.quit) }) }

// reload brings the running plugins in line with the approvals: new ones
// start, revoked or re-approved ones stop (and the re-approved start again).
func (b *broker) reload() {
	approved := plugin.Enabled()
	b.mu.Lock()
	var stopping []*runner
	for name, r := range b.plugins {
		if a, ok := approved[name]; !ok || a.Digest != r.digest {
			stopping = append(stopping, r)
			delete(b.plugins, name)
		}
	}
	for name, a := range approved {
		if _, ok := b.plugins[name]; !ok {
			r := newRunner(b, name, a.Digest)
			b.plugins[name] = r
			go r.supervise()
		}
	}
	empty := len(b.plugins) == 0
	b.mu.Unlock()
	// A revoked plugin's arrangement of the list goes with it.
	plugin.PruneSidebars(approved)
	for _, r := range stopping {
		b.log.Printf("%s: stopping (approval changed)", r.name)
		r.shutdown()
	}
	if empty {
		b.log.Printf("no plugins to run")
		b.stop()
	}
}

func (b *broker) runner(name string) *runner {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.plugins[name]
}

// watch checks every few seconds that each plugin is within its memory
// limit and its files are as approved.
func (b *broker) watch() {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for tick := 0; ; tick++ {
		select {
		case <-b.quit:
			return
		case <-t.C:
		}
		b.mu.Lock()
		rs := make([]*runner, 0, len(b.plugins))
		for _, r := range b.plugins {
			rs = append(rs, r)
		}
		b.mu.Unlock()
		var pids []int
		for _, r := range rs {
			if pid := r.pidNow(); pid > 0 {
				pids = append(pids, pid)
			}
		}
		if len(pids) > 0 {
			table := proc.Snapshot(nil)
			table.Fill(nil, pids)
			for _, r := range rs {
				pid := r.pidNow()
				p := table.Procs[pid]
				if p == nil {
					continue
				}
				if limit := r.p.Memory(); p.Footprint > limit {
					r.fail(fmt.Sprintf("using %d MB, over its %d MB limit", p.Footprint>>20, limit>>20))
				}
			}
		}
		// Files changed under a running plugin: it is no longer the one you
		// approved. Checked less often, as it reads every file.
		if tick%6 == 5 {
			for _, r := range rs {
				if r.pidNow() > 0 && !r.p.Bundled {
					if d, err := plugin.Digest(r.p.Dir); err != nil || d != r.digest {
						r.fail("its files changed since it was approved")
					}
				}
			}
		}
	}
}

// fromRush answers rush itself: session hosts and the CLI.
func (b *broker) fromRush(ctx context.Context, method string, params jsontext.Value) (any, error) {
	switch method {
	case "mcp":
		var p struct {
			Plugin  string         `json:"plugin"`
			Session string         `json:"session"`
			Message jsontext.Value `json:"message"`
		}
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: err.Error()}
		}
		r := b.runner(p.Plugin)
		if r == nil {
			return nil, fmt.Errorf("%s is not running", p.Plugin)
		}
		return r.mcp(ctx, p.Session, p.Message), nil
	case "cli":
		var p struct {
			Plugin string        `json:"plugin"`
			Run    plugin.CLIRun `json:"run"`
		}
		if err := jsonx.Unmarshal(params, &p); err != nil {
			return nil, &plugin.Error{Code: plugin.CodeInvalidParams, Message: err.Error()}
		}
		return b.cli(ctx, p.Plugin, p.Run)
	case "reload":
		go b.reload()
		return map[string]any{}, nil
	case "status":
		return b.status(), nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// cli runs one of a plugin's CLI commands, for `rush <plugin> <command>`.
func (b *broker) cli(ctx context.Context, name string, run plugin.CLIRun) (plugin.CLIResult, error) {
	r := b.runner(name)
	if r == nil {
		b.reload() // a broker the CLI just started
		if r = b.runner(name); r == nil {
			return plugin.CLIResult{}, fmt.Errorf("%s is not running", name)
		}
	}
	conn, err := r.wait(ctx)
	if err != nil {
		return plugin.CLIResult{}, err
	}
	p, _ := r.live()
	if err := p.CheckCLIRun(run); err != nil {
		return plugin.CLIResult{}, &plugin.Error{Code: plugin.CodeInvalidParams, Message: err.Error()}
	}
	ctx, cancel := context.WithTimeout(ctx, plugin.CLITimeout)
	defer cancel()
	var out plugin.CLIResult
	if err := conn.Call(ctx, "cli.run", run, &out); err != nil {
		return plugin.CLIResult{}, err
	}
	return out.Clip(), nil
}

// Status is what the broker says about one plugin.
type Status struct {
	Name     string    `json:"name"`
	State    string    `json:"state"` // starting, running, waiting, refused
	PID      int       `json:"pid,omitzero"`
	Since    time.Time `json:"since"`
	Restarts int       `json:"restarts,omitzero"`
	Error    string    `json:"error,omitempty"`
}

func (b *broker) status() []Status {
	b.mu.Lock()
	rs := make([]*runner, 0, len(b.plugins))
	for _, r := range b.plugins {
		rs = append(rs, r)
	}
	b.mu.Unlock()
	out := make([]Status, 0, len(rs))
	for _, r := range rs {
		out = append(out, r.status())
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// Ask asks the running broker for its plugins' status.
func Ask() ([]Status, error) {
	c, err := plugin.DialBroker()
	if err != nil {
		return nil, err
	}
	defer c.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	var out []Status
	return out, c.Call(ctx, "status", nil, &out)
}

// Reload tells the running broker the approvals changed, starting it if it
// isn't running and a plugin is approved.
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

// kill ends a plugin's process group: politely, then not.
func kill(cmd *exec.Cmd, exited <-chan struct{}) {
	pid := cmd.Process.Pid
	_ = syscall.Kill(-pid, syscall.SIGTERM)
	select {
	case <-exited:
	case <-time.After(2 * time.Second):
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		<-exited
	}
}

var errStopped = errors.New("stopped")
