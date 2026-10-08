package plugind

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"log"
	"maps"
	"net"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// runner keeps one plugin running.
type runner struct {
	b      *broker
	name   string
	digest string // as approved
	log    *log.Logger

	mu       sync.Mutex
	p        plugin.Plugin
	conn     *plugin.Conn
	ready    chan struct{} // closed while conn is up
	cmd      *exec.Cmd
	exited   chan struct{}
	pid      int
	mcpInit  jsontext.Value // an MCP plugin's initialize result
	state    string
	since    time.Time
	restarts int
	lastErr  string
	subs     map[string]*sub // followed sessions, by id
	unwatch  func()          // ends sessions.watch
	out      *outbox         // notifications to it, while it runs
	execs    chan struct{}   // programs it's running, see exec
	quit     chan struct{}
	quitOnce sync.Once

	// starts is when it last started sessions, to cap how fast it can.
	starts []time.Time
}

func newRunner(b *broker, name, digest string) *runner {
	return &runner{b: b, name: name, digest: digest, log: log.New(os.Stderr, "plugind "+name+" ", log.LstdFlags),
		ready: make(chan struct{}), state: "starting", since: time.Now(), subs: map[string]*sub{},
		execs: make(chan struct{}, maxExecs), quit: make(chan struct{})}
}

func (r *runner) status() Status {
	r.mu.Lock()
	defer r.mu.Unlock()
	return Status{Name: r.name, State: r.state, PID: r.pid, Since: r.since, Restarts: r.restarts, Error: r.lastErr}
}

func (r *runner) pidNow() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.pid
}

func (r *runner) setState(s, err string) {
	r.mu.Lock()
	r.state, r.since = s, time.Now()
	if err != "" {
		r.lastErr = err
	}
	r.mu.Unlock()
}

// supervise starts the plugin and starts it again whenever it ends, until
// it is shut down or refused.
func (r *runner) supervise() {
	backoff := time.Second
	for {
		select {
		case <-r.quit:
			return
		default:
		}
		p, err := plugin.Verify(r.name)
		if err == nil && !p.Bundled {
			err = plugin.Supported()
		}
		if err != nil {
			// Not as approved: it waits for you, not for a timer.
			r.log.Printf("refused: %v", err)
			r.setState("refused", err.Error())
			return
		}
		began := time.Now()
		err = r.run(p)
		select {
		case <-r.quit:
			return
		default:
		}
		if time.Since(began) > time.Minute {
			backoff = time.Second
		}
		msg := "exited"
		if err != nil {
			msg = err.Error()
		}
		r.log.Printf("%s; starting again in %s", msg, backoff)
		r.mu.Lock()
		r.restarts++
		r.mu.Unlock()
		r.setState("waiting", msg)
		select {
		case <-r.quit:
			return
		case <-time.After(backoff):
		}
		backoff = min(backoff*2, time.Minute)
	}
}

// run starts the plugin, and returns when it has exited.
func (r *runner) run(p plugin.Plugin) error {
	r.mu.Lock()
	r.p = p
	r.mu.Unlock()
	r.setState("starting", "")

	var proxy *plugin.Proxy
	if len(p.Network) > 0 {
		var err error
		if proxy, err = plugin.Listen(p.Network, r.log); err != nil {
			return err
		}
		defer proxy.Close()
	}
	l := plugin.Launch{Plugin: p}
	if proxy != nil {
		l.ProxyPort = proxy.Port()
	}
	cmd, err := l.Process()
	if err != nil {
		return err
	}
	logf, err := os.OpenFile(plugin.LogPath(p.Name), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer logf.Close()
	cmd.Stderr = logf

	var conn *plugin.Conn
	switch p.Proto() {
	case plugin.ProtoMCP:
		stdin, err := cmd.StdinPipe()
		if err != nil {
			return err
		}
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			return err
		}
		if err := cmd.Start(); err != nil {
			return err
		}
		conn = plugin.NewLineConn(stdout, stdin, fromMCPServer)
	default:
		cmd.Stdout = logf
		mine, theirs, err := socketPair()
		if err != nil {
			return err
		}
		cmd.ExtraFiles = []*os.File{theirs} // fd 3
		err = cmd.Start()
		theirs.Close()
		if err != nil {
			mine.Close()
			return err
		}
		nc, err := net.FileConn(mine)
		mine.Close()
		if err != nil {
			_ = cmd.Process.Kill()
			_ = cmd.Wait()
			return err
		}
		conn = plugin.NewConn(nc, r.fromPlugin)
	}

	exited := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(exited) }()
	r.mu.Lock()
	r.cmd, r.exited, r.pid = cmd, exited, cmd.Process.Pid
	r.mu.Unlock()
	defer r.down(conn)

	if err := r.handshake(p, conn); err != nil {
		kill(cmd, exited)
		return fmt.Errorf("did not start: %w", err)
	}
	r.mu.Lock()
	r.conn = conn
	if p.Proto() == plugin.ProtoRush {
		r.out = newOutbox(conn, pluginQueue)
	}
	close(r.ready)
	r.mu.Unlock()
	r.setState("running", "")
	if r.b != nil && r.b.ui != nil {
		r.b.ui.changed()
	}
	r.log.Printf("running, pid %d", cmd.Process.Pid)

	select {
	case <-exited:
	case <-conn.Done():
		// It closed its end of the channel; it's no use without it.
		kill(cmd, exited)
	case <-r.quit:
		kill(cmd, exited)
		return errStopped
	}
	return waitErr
}

// down forgets a run that ended: its connection, what it followed, and what
// it added to rush's screen.
func (r *runner) down(conn *plugin.Conn) {
	conn.Close()
	r.mu.Lock()
	r.conn, r.cmd, r.pid, r.mcpInit = nil, nil, 0, nil
	r.ready = make(chan struct{})
	r.out.close()
	r.out = nil
	for id, s := range r.subs {
		s.cancel()
		delete(r.subs, id)
	}
	if r.unwatch != nil {
		r.unwatch()
		r.unwatch = nil
	}
	r.mu.Unlock()
	// What it added to rush's screen goes with it.
	if r.b != nil && r.b.ui != nil {
		r.b.ui.gone(r.name)
	}
}

// socketPair is a connected pair of unix sockets, neither passed on to any
// other process by accident.
func socketPair() (mine, theirs *os.File, err error) {
	syscall.ForkLock.RLock()
	fds, err := syscall.Socketpair(syscall.AF_UNIX, syscall.SOCK_STREAM, 0)
	if err == nil {
		syscall.CloseOnExec(fds[0])
		syscall.CloseOnExec(fds[1])
	}
	syscall.ForkLock.RUnlock()
	if err != nil {
		return nil, nil, err
	}
	return os.NewFile(uintptr(fds[0]), "plugin-ipc"), os.NewFile(uintptr(fds[1]), "plugin-ipc"), nil
}

// handshake says hello. A rush plugin is told who it is and what it may
// do; an MCP server is initialized as any MCP client would.
func (r *runner) handshake(p plugin.Plugin, conn *plugin.Conn) error {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if p.Proto() == plugin.ProtoMCP {
		res, err := conn.CallRaw(ctx, "initialize", mustJSON(map[string]any{
			"protocolVersion": "2025-06-18", "capabilities": map[string]any{},
			"clientInfo": map[string]any{"name": "rush", "version": strconv.Itoa(Version)},
		}))
		if err != nil {
			return err
		}
		r.mu.Lock()
		r.mcpInit = res
		r.mu.Unlock()
		return conn.Notify("notifications/initialized", nil)
	}
	return conn.Call(ctx, "initialize", map[string]any{
		"protocol": Version, "name": p.Name, "dataDir": plugin.DataDir(p.Name),
		"sessions": p.Sessions, "workspaces": p.WorkspaceDirs(), "network": p.Network,
		"exec": slices.Sorted(maps.Keys(p.Exec)), "ui": p.UI, "commands": commandNames(p.Commands),
		"settings": plugin.SettingValuesOf(&p.Manifest),
	}, nil)
}

func commandNames(cs []plugin.CommandSpec) []string {
	out := make([]string, 0, len(cs))
	for _, c := range cs {
		out = append(out, c.Name)
	}
	return out
}

// live is the plugin's manifest, and its connection if it's up.
func (r *runner) live() (plugin.Plugin, *plugin.Conn) {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.p, r.conn
}

// notify queues a notification to the plugin, without waiting: when the
// plugin falls behind, the oldest waiting is dropped, and one with a key
// replaces the one waiting with the same key.
func (r *runner) notify(method string, params any, key string) {
	r.mu.Lock()
	out := r.out
	r.mu.Unlock()
	out.put(method, params, key)
}

// fromMCPServer answers what an MCP server asks of its client: nothing it
// asks for (sampling, roots, elicitation) is offered.
func fromMCPServer(_ context.Context, method string, _ jsontext.Value) (any, error) {
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "rush does not offer " + method}
}

// shutdown stops it for good.
func (r *runner) shutdown() {
	r.quitOnce.Do(func() { close(r.quit) })
	r.mu.Lock()
	exited := r.exited
	r.mu.Unlock()
	if exited != nil {
		<-exited
	}
}

// fail ends the running process for a reason; supervise starts it again.
func (r *runner) fail(why string) {
	r.mu.Lock()
	cmd, exited := r.cmd, r.exited
	r.lastErr = why
	r.mu.Unlock()
	if cmd == nil {
		return
	}
	r.log.Printf("ending it: %s", why)
	kill(cmd, exited)
}

// wait returns the connection once the plugin is up. A plugin that isn't
// up within a few seconds is failing to start, and the call fails rather
// than wait out the backoff.
func (r *runner) wait(ctx context.Context) (*plugin.Conn, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	for {
		r.mu.Lock()
		ready, conn, state := r.ready, r.conn, r.state
		r.mu.Unlock()
		if conn != nil {
			return conn, nil
		}
		if state == "refused" {
			return nil, fmt.Errorf("%s is not running: %s", r.name, r.status().Error)
		}
		select {
		case <-ready:
		case <-ctx.Done():
			msg := r.name + " is not running"
			if e := r.status().Error; e != "" {
				msg += ": " + e + " (see rush plugin logs " + r.name + ")" //nolint:rush // once, then it returns
			}
			return nil, errors.New(msg)
		case <-time.After(time.Second):
		}
	}
}

// mcp answers one MCP message from a session for this plugin's server,
// with the whole JSON-RPC reply.
func (r *runner) mcp(ctx context.Context, session string, msg jsontext.Value) jsontext.Value {
	var m struct {
		ID     jsontext.Value `json:"id"`
		Method string         `json:"method"`
		Params jsontext.Value `json:"params"`
	}
	if err := jsonx.Unmarshal(msg, &m); err != nil {
		return rpcReply(nil, nil, &plugin.Error{Code: plugin.CodeParse, Message: "parse error"})
	}
	r.mu.Lock()
	p := r.p
	r.mu.Unlock()
	if !p.HasTools() {
		return rpcReply(m.ID, nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: r.name + " has no tools"})
	}
	var res jsontext.Value
	var err error
	switch m.Method {
	case "initialize":
		var in struct {
			ProtocolVersion string `json:"protocolVersion"`
		}
		_ = jsonx.Unmarshal(m.Params, &in)
		out := map[string]any{
			"protocolVersion": in.ProtocolVersion,
			"capabilities":    map[string]any{"tools": map[string]any{}},
			"serverInfo":      map[string]any{"name": p.Server(), "version": p.Version},
		}
		// An MCP server's own guidance for using it goes along.
		if p.Proto() == plugin.ProtoMCP {
			if _, werr := r.wait(ctx); werr == nil {
				var init struct {
					Instructions string `json:"instructions"`
				}
				r.mu.Lock()
				_ = jsonx.Unmarshal(r.mcpInit, &init)
				r.mu.Unlock()
				if init.Instructions != "" {
					out["instructions"] = init.Instructions
				}
			}
		}
		res = mustJSON(out)
	case "ping":
		res = jsontext.Value(`{}`)
	case "tools/list":
		var conn *plugin.Conn
		if conn, err = r.wait(ctx); err == nil {
			if p.Proto() == plugin.ProtoMCP {
				res, err = conn.CallRaw(ctx, "tools/list", m.Params)
			} else {
				res, err = conn.CallRaw(ctx, "tools.list", nil)
			}
		}
	case "tools/call":
		var conn *plugin.Conn
		if conn, err = r.wait(ctx); err == nil {
			if p.Proto() == plugin.ProtoMCP {
				res, err = conn.CallRaw(ctx, "tools/call", m.Params)
			} else {
				var call struct {
					Name      string         `json:"name"`
					Arguments jsontext.Value `json:"arguments"`
				}
				_ = jsonx.Unmarshal(m.Params, &call)
				args := map[string]any{"session": session, "name": call.Name, "arguments": call.Arguments}
				// Who's calling, so a plugin can tie the call to what it
				// knows the session by.
				if idRE.MatchString(session) {
					if i, ierr := host.ReadInfo(session); ierr == nil {
						args["sessionId"], args["cwd"], args["meta"] = i.SessionID, i.Cwd, i.Meta
					}
				}
				res, err = conn.CallRaw(ctx, "tools.call", mustJSON(args))
			}
		}
	default:
		err = &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + m.Method}
	}
	if err != nil {
		e, ok := err.(*plugin.Error)
		if !ok {
			e = &plugin.Error{Code: plugin.CodeServer, Message: err.Error()}
		}
		return rpcReply(m.ID, nil, e)
	}
	return rpcReply(m.ID, res, nil)
}

func rpcReply(id, result jsontext.Value, e *plugin.Error) jsontext.Value {
	out := map[string]any{"jsonrpc": "2.0"}
	if id != nil {
		out["id"] = id
	}
	if e != nil {
		out["error"] = e
	} else {
		out["result"] = result
	}
	return mustJSON(out)
}

func mustJSON(v any) jsontext.Value {
	b, _ := jsonx.Marshal(v)
	return b
}
