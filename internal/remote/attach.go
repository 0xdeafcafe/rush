package remote

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Attach makes another machine's sessions part of this machine's rush: each
// shows in the list and opens in the view like a local one, and is driven
// through that machine's serve. It does it by standing in for their hosts:
// for every remote session it keeps a session folder, with an info file
// and a host socket, that speaks the host protocol on one side and serve's
// HTTP on the other. The view needs no change, and nothing runs here: the
// agent, its files and its tools stay on the other machine.
//
// What it can't be, it says: only the ops serve allows pass (see
// remoteOps); each stand-in carries Info.Remote, which the view and the host
// package check before any local action (signals, restarts, wakes, folders);
// its working folder is under a folder of rush's own that nothing can enter
// (see unreachable), so a path that also exists here is never the session's;
// and history is what serve replays, not a transcript read from disk.
// Attach runs until ctx ends, then removes what it made, and only that: a
// folder is its own only if it holds the marker it wrote (see marker).
func Attach(ctx context.Context, p Peer, log io.Writer) error {
	// One attach per machine: a second would take over the first's sockets.
	lock, err := os.OpenFile(filepath.Join(state.Dir(), "attach-"+p.Name+".lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		return fmt.Errorf("already attached to %s by another rush", p.Name)
	}
	if err := makeUnreachable(p.Name); err != nil {
		return err
	}
	b := &bridge{p: p, log: log, pid: os.Getpid(), sess: map[string]*alias{}, client: &http.Client{}}
	if err := os.MkdirAll(host.Root(), 0o700); err != nil {
		return err
	}
	b.purge()
	defer b.close()
	defer removeUnreachable(p.Name)
	tick := time.NewTicker(3 * time.Second)
	defer tick.Stop()
	for {
		if err := b.sync(ctx); err != nil && ctx.Err() == nil {
			fmt.Fprintf(log, "rush remote attach: %s: %v\n", p.Name, err)
		}
		select {
		case <-ctx.Done():
			return nil
		case <-tick.C:
		}
	}
}

// attachPeers runs Attach for each of cfg's peers while the remote-client-tui
// plugin is on, so their sessions are in this machine's rush with no terminal
// running rush remote attach; off, or once ctx ends, it ends them. A peer whose
// attach ends (another rush has it, or it failed) is tried again a little later.
func attachPeers(ctx context.Context, cfg Config, log io.Writer) {
	type running struct {
		stop context.CancelFunc
		done chan struct{}
	}
	on := map[string]*running{}
	retry := map[string]time.Time{}
	tick := time.NewTicker(3 * time.Second)
	defer tick.Stop()
	for {
		want := ctx.Err() == nil && plugin.BundledOn(PluginTUI)
		for _, p := range cfg.Peers {
			r := on[p.Name]
			if r != nil {
				select {
				case <-r.done:
					delete(on, p.Name)
					retry[p.Name], r = time.Now().Add(30*time.Second), nil
				default:
				}
			}
			switch {
			case want && r == nil && time.Now().After(retry[p.Name]):
				c, stop := context.WithCancel(ctx)
				r = &running{stop: stop, done: make(chan struct{})}
				on[p.Name] = r
				go func() {
					defer close(r.done)
					if err := Attach(c, p, log); err != nil && c.Err() == nil {
						fmt.Fprintf(log, "rush serve: attach %s: %v\n", p.Name, err)
					}
				}()
			case !want && r != nil:
				r.stop()
				<-r.done
				delete(on, p.Name)
			}
		}
		if ctx.Err() != nil {
			return
		}
		select {
		case <-ctx.Done():
		case <-tick.C:
		}
	}
}

// alias is one remote session as a local one.
type alias struct {
	id    string // the local id
	rid   string // the remote's
	dir   string
	token string // what its marker holds: ownership is this, never the id
	ln    net.Listener
	last  []byte // info.json as last written
}

// marker is bridge.json in an alias's folder: the proof an attach made the
// folder, for which machine and session, and which attach (pid) is meant to
// be keeping it. Nothing is changed or removed in a folder without it, and
// nothing a remote or a plugin says (Meta, an Info field) stands for it.
type marker struct {
	Machine  string `json:"machine"`
	RemoteID string `json:"remoteId"`
	PID      int    `json:"pid"`
	Token    string `json:"token"`
}

const markerName = "bridge.json"

type bridge struct {
	p      Peer
	log    io.Writer
	pid    int
	client *http.Client
	mu     sync.Mutex
	sess   map[string]*alias // by remote id
}

// localID is the n-th id a remote session may have here: eight hex digits
// like any, from the machine, its own id and n. Eight digits can meet
// another session's, so an id is never taken as free: it's reserved by making
// its folder (see reserve), and the next n is tried when that fails.
func localID(machine, rid string, n int) string {
	h := sha256.Sum256([]byte(fmt.Sprintf("%s/%s/%d", machine, rid, n)))
	return hex.EncodeToString(h[:4])
}

// unreachable is the folder remote sessions' working folders are shown
// under: a folder of rush's own, made with no permissions at all, so no
// path under it can be read, listed, entered or run in (git, an editor, a
// shell), and no real project is ever behind one. A guarantee of the file
// system's, not a naming convention.
func unreachable(machine string) string {
	dir, err := filepath.Abs(filepath.Join(state.Dir(), "remote-cwd", machine))
	if err != nil {
		dir = filepath.Join(state.Dir(), "remote-cwd", machine)
	}
	return dir
}

func makeUnreachable(machine string) error {
	dir := unreachable(machine)
	if err := os.MkdirAll(filepath.Dir(dir), 0o700); err != nil {
		return err
	}
	if err := os.Mkdir(dir, 0o000); err != nil && !errors.Is(err, fs.ErrExist) {
		return err
	}
	return os.Chmod(dir, 0o000)
}

func removeUnreachable(machine string) {
	dir := unreachable(machine)
	_ = os.Chmod(dir, 0o700)
	_ = os.Remove(dir) // empty, or not removed
}

// localInfo is a remote session's info as this machine shows it, as the
// stand-in with id: marked remote, with the attach's pid (what's alive here)
// and nothing of the agent's own processes or folders.
func localInfo(machine, id string, pid int, i host.Info) host.Info {
	rid := i.ID
	i.ID = id
	i.Remote, i.RemoteID = machine, rid
	i.HostPID = pid
	i.ClaudePID = 0
	i.Sleeping = false // waking is the remote's, on a send
	i.Cwd = filepath.Join(unreachable(machine), i.Cwd)
	i.CwdAt = time.Time{}
	i.TempDir, i.Left = "", nil
	if i.Name == "" {
		i.Name = rid
	}
	i.Name += " @" + machine
	return i
}

// readMarker is dir's marker, if dir is a real folder (not a link to one)
// that has one.
func readMarker(dir string) (marker, bool) {
	var m marker
	if st, err := os.Lstat(dir); err != nil || !st.IsDir() {
		return m, false
	}
	raw, err := os.ReadFile(filepath.Join(dir, markerName))
	if err != nil || jsonx.Unmarshal(raw, &m) != nil || m.Token == "" {
		return m, false
	}
	return m, true
}

// purge removes what an attach to this machine that didn't end cleanly left:
// folders that hold this machine's marker of an attach no longer running.
func (b *bridge) purge() {
	ents, _ := os.ReadDir(host.Root())
	for _, e := range ents {
		dir := filepath.Join(host.Root(), e.Name())
		if m, ok := readMarker(dir); ok && m.Machine == b.p.Name && m.PID != b.pid && !host.Alive(m.PID) {
			_ = os.RemoveAll(dir)
		}
	}
}

func (b *bridge) close() {
	b.mu.Lock()
	defer b.mu.Unlock()
	for rid, a := range b.sess {
		b.drop(rid, a)
	}
}

// owns is whether a's folder is still the one it made: its marker, its token.
func (a *alias) owns() bool {
	m, ok := readMarker(a.dir)
	return ok && m.Token == a.token
}

// drop ends a, removing its folder only if it still owns it.
func (b *bridge) drop(rid string, a *alias) {
	if u, ok := a.ln.(*net.UnixListener); ok {
		u.SetUnlinkOnClose(false) // the path may not be ours any more
	}
	_ = a.ln.Close()
	if a.owns() {
		_ = os.RemoveAll(a.dir)
	}
	delete(b.sess, rid)
}

// reserve makes a folder for remote session rid under the first id nobody
// has. Mkdir is the claim: it fails if anything is there (a session, a link,
// a file), so nothing of anyone's is touched; a folder that holds this
// machine's marker of an attach that has ended is its own stale one, taken
// back. The marker is written, and the socket made, only in a folder this
// call made.
func (b *bridge) reserve(rid string) (*alias, error) {
	for n := 0; n < 100; n++ {
		id := localID(b.p.Name, rid, n)
		dir := filepath.Join(host.Root(), id)
		err := os.Mkdir(dir, 0o700)
		if errors.Is(err, fs.ErrExist) {
			m, ok := readMarker(dir)
			if !ok || m.Machine != b.p.Name || m.RemoteID != rid || m.PID == b.pid || host.Alive(m.PID) {
				continue // somebody's: take the next id
			}
			if os.RemoveAll(dir) != nil || os.Mkdir(dir, 0o700) != nil {
				continue
			}
			err = nil
		}
		if err != nil {
			return nil, err
		}
		a := &alias{id: id, rid: rid, dir: dir, token: hex.EncodeToString(randBytes(16))}
		raw, _ := jsonx.Marshal(marker{Machine: b.p.Name, RemoteID: rid, PID: b.pid, Token: a.token})
		if err := os.WriteFile(filepath.Join(dir, markerName), raw, 0o600); err != nil {
			_ = os.RemoveAll(dir) // made a moment ago, by this call
			return nil, err
		}
		ln, err := net.Listen("unix", host.SockPath(id))
		if err != nil {
			_ = os.RemoveAll(dir)
			return nil, err
		}
		a.ln = ln
		return a, nil
	}
	return nil, errors.New("no free session id")
}

func randBytes(n int) []byte {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return b
}

// sync reads the remote's sessions and brings the local folders to match.
func (b *bridge) sync(ctx context.Context) error {
	var list []Session
	cctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	if err := peerGet(cctx, b.p, "/api/sessions", &list); err != nil {
		return err
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	seen := map[string]bool{}
	for _, s := range list {
		if !validRemoteID(s.ID) {
			continue
		}
		seen[s.ID] = true
		a := b.sess[s.ID]
		if a == nil {
			var err error
			if a, err = b.reserve(s.ID); err != nil {
				fmt.Fprintf(b.log, "rush remote attach: %s: %v\n", s.ID, err)
				continue
			}
			b.sess[s.ID] = a
			b.serve(ctx, a)
		}
		info := localInfo(b.p.Name, a.id, b.pid, s.Info)
		if !s.Alive {
			info.State = "stopped"
		}
		raw, _ := jsonx.Marshal(info)
		if bytes.Equal(raw, a.last) {
			continue
		}
		if !a.owns() { // the folder was replaced under us: touch nothing in it
			fmt.Fprintf(b.log, "rush remote attach: %s's folder is no longer ours; leaving it\n", a.id)
			b.drop(s.ID, a)
			continue
		}
		tmp := filepath.Join(a.dir, "info.json.tmp")
		if os.WriteFile(tmp, raw, 0o600) == nil && os.Rename(tmp, filepath.Join(a.dir, "info.json")) == nil {
			a.last = raw
		}
	}
	for rid, a := range b.sess {
		if !seen[rid] {
			b.drop(rid, a)
		}
	}
	return nil
}

func validRemoteID(id string) bool {
	return id != "" && !strings.ContainsAny(id, `/\.?#% `)
}

// serve starts answering on a's socket.
func (b *bridge) serve(ctx context.Context, a *alias) {
	go func() {
		for {
			c, err := a.ln.Accept()
			if err != nil {
				return
			}
			go b.conn(ctx, a, c)
		}
	}()
}

// conn is one view's connection to a session: what the remote streams goes
// to it as host lines, what it sends goes to the remote as ops.
func (b *bridge) conn(ctx context.Context, a *alias, c net.Conn) {
	defer c.Close()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var wmu sync.Mutex
	write := func(line []byte) bool {
		wmu.Lock()
		defer wmu.Unlock()
		_, err := c.Write(append(line, '\n'))
		return err == nil
	}
	go func() { // the view's ops
		defer cancel()
		r := jsonx.NewLineReader(c)
		for {
			l, ok := r.Next()
			if !ok {
				return
			}
			if err := b.op(ctx, a.rid, l); err != nil {
				write(hostError(err.Error()))
			}
		}
	}()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, b.p.URL+"/api/sessions/"+a.rid+"/events?full=1", nil) // a view takes all of it, as it would a local transcript
	if err != nil {
		return
	}
	req.Header.Set("Authorization", "Bearer "+b.p.Token)
	resp, err := b.client.Do(req)
	if err != nil {
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return
	}
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(nil, 64<<20)
	resets := 0
	for sc.Scan() {
		data, ok := strings.CutPrefix(sc.Text(), "data: ")
		if !ok {
			continue
		}
		out, reset := hostLine(b.p.Name, a.id, b.pid, []byte(data))
		if reset {
			// The remote started over (its host went or came): a view that
			// redials gets a clean replay, which is all a view can take.
			if resets++; resets > 1 {
				return
			}
			continue
		}
		if out != nil && !write(out) {
			return
		}
	}
}

// op passes one of the view's host ops to the remote, if serve allows it.
func (b *bridge) op(ctx context.Context, rid string, line []byte) error {
	name := host.OpName(line)
	if name == "hello" || name == "" {
		return nil
	}
	if !slices.Contains(remoteOps, name) {
		return fmt.Errorf("%s isn't available on a session on %s", name, b.p.Name)
	}
	var o remoteOp // only what serve takes: never a path on this machine
	if err := jsonx.Unmarshal(line, &o); err != nil {
		return err
	}
	var withImages struct {
		Images []string `json:"images"`
	}
	_ = jsonx.Unmarshal(line, &withImages)
	pics, err := readPictures(withImages.Images)
	if err != nil {
		return err
	}
	o.Pictures = pics
	body, _ := jsonx.Marshal(o)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, b.p.URL+"/api/sessions/"+rid+"/op", bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+b.p.Token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := b.client.Do(req)
	if err != nil {
		return fmt.Errorf("%s can't be reached: %w", b.p.Name, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		var e struct{ Error string }
		raw, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<16))
		_ = jsonx.Unmarshal(raw, &e)
		return errors.New(or(e.Error, resp.Status))
	}
	return nil
}

// readPictures reads the images a view attached, by their paths here, to
// send as bytes: the path stays on this machine.
func readPictures(paths []string) ([]Picture, error) {
	var out []Picture
	for _, p := range paths {
		var typ string
		for t, ext := range pictureExt {
			if e := strings.ToLower(filepath.Ext(p)); e == ext || ext == ".jpg" && e == ".jpeg" {
				typ = t
			}
		}
		if typ == "" {
			return nil, fmt.Errorf("%s isn't a png, jpeg, gif or webp image", filepath.Base(p))
		}
		st, err := os.Stat(p)
		if err != nil {
			return nil, err
		}
		if st.Size() > pictureMost {
			return nil, fmt.Errorf("%s is %d MB; images must be under %d MB", filepath.Base(p), st.Size()>>20, pictureMost>>20)
		}
		b, err := os.ReadFile(p)
		if err != nil {
			return nil, err
		}
		out = append(out, Picture{Type: typ, Data: b})
	}
	return out, nil
}

func hostError(msg string) []byte {
	b, _ := jsonx.Marshal(map[string]string{"type": "agtop_error", "error": msg})
	return b
}

// hostLine is one of serve's event lines ({"t":…,"e":…}) as the line a
// host would have sent, nil for ones with no use; reset says the stream
// began again.
func hostLine(machine, id string, pid int, data []byte) (line []byte, reset bool) {
	var w struct {
		T string         `json:"t"`
		E jsontext.Value `json:"e"`
	}
	if jsonx.Unmarshal(data, &w) != nil {
		return nil, false
	}
	switch w.T {
	case "reset":
		return nil, true
	case "cut", "":
		return nil, false
	case "info":
		var i host.Info
		if jsonx.Unmarshal(w.E, &i) != nil {
			return nil, false
		}
		raw, _ := jsonx.Marshal(localInfo(machine, id, pid, i))
		return append(append([]byte(`{"info":`), raw...), []byte(`,"type":"agtop_info"}`)...), false
	case "error":
		var e host.ErrorEvent
		_ = jsonx.Unmarshal(w.E, &e)
		return hostError(e.Error), false
	case "answered":
		var a host.Answered
		_ = jsonx.Unmarshal(w.E, &a)
		raw, _ := jsonx.Marshal(map[string]string{"type": "agtop_answered", "request_id": a.ID})
		return raw, false
	case "stamp":
		var s host.Stamp
		if jsonx.Unmarshal(w.E, &s) != nil {
			return nil, false
		}
		raw, _ := jsonx.Marshal(map[string]any{"type": "agtop_time", "t": s.At.UnixMilli()})
		return raw, false
	case "sent":
		var s host.Sent
		if jsonx.Unmarshal(w.E, &s) != nil {
			return nil, false
		}
		m := map[string]any{"type": "user", "message": map[string]any{"role": "user", "content": s.Text}, "agtop_sent": true}
		if s.Exchange != nil {
			m["agtop_exchange"] = s.Exchange
		}
		raw, _ := jsonx.Marshal(m)
		return raw, false
	}
	if _, err := event.Unmarshal(data); err != nil {
		return nil, false
	}
	return append(append([]byte(`{"type":"agtop_ev","ev":`), data...), '}'), false
}
