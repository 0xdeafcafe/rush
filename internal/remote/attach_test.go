package remote

import (
	"context"
	"io"
	"io/fs"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestHostLine(t *testing.T) {
	shortHome(t)
	info := host.Info{ID: "abcd1234", Cwd: "/srv/p", Name: "job", State: "working", Sleeping: true, HostPID: 1, ClaudePID: 2, Meta: map[string]string{"k": "v"}}
	raw := `{"t":"info","e":` + string(must(t, info)) + `}`
	out, _ := hostLine("box", "deadbeef", 4242, []byte(raw))
	ev, err := new(host.Decoder).Decode(out)
	if err != nil || len(ev) != 1 {
		t.Fatalf("%s: %v %v", out, ev, err)
	}
	got := ev[0].(host.InfoEvent).Info
	if got.ID != "deadbeef" || got.HostPID != 4242 || got.ClaudePID != 0 || got.Sleeping || !got.IsRemote() ||
		got.Remote != "box" || got.RemoteID != "abcd1234" || got.Name != "job @box" || got.Meta["k"] != "v" ||
		got.Cwd != filepath.Join(unreachable("box"), "/srv/p") {
		t.Fatalf("%+v", got)
	}
	// an event, a sent echo, an answer and the stream starting over
	line := evLine(t, event.Approval{ID: "a1"})
	b, _ := event.Marshal(event.Approval{ID: "a1"})
	if out, _ := hostLine("box", "x", 1, b); string(out) != string(line) {
		t.Errorf("event: %s", out)
	}
	if out, _ := hostLine("box", "x", 1, []byte(`{"t":"sent","e":{"Text":"hi"}}`)); !strings.Contains(string(out), `"agtop_sent":true`) {
		t.Errorf("sent: %s", out)
	}
	if out, _ := hostLine("box", "x", 1, []byte(`{"t":"answered","e":{"ID":"a1"}}`)); !strings.Contains(string(out), `"request_id":"a1"`) {
		t.Errorf("answered: %s", out)
	}
	if _, reset := hostLine("box", "x", 1, []byte(`{"t":"reset","e":null}`)); !reset {
		t.Error("reset")
	}
}

func must(t *testing.T, v any) []byte {
	b, err := jsonx.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func freePort(t *testing.T) int {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	return ln.Addr().(*net.TCPAddr).Port
}

// The native client end to end, as two machines: a `rush serve` with a fake
// host in one home, and `rush remote attach` in another. A view here (the
// host client the TUI uses) then lists, opens and drives that session, and
// can't do what the remote doesn't allow.
func TestAttachDrivesARemoteSession(t *testing.T) {
	if testing.Short() {
		t.Skip("builds rush")
	}
	bin := filepath.Join(t.TempDir(), "rush")
	build := exec.Command("go", "build", "-o", bin, "github.com/0xdeafcafe/rush/cmd/rush")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	remoteHome, localHome := t.TempDir(), t.TempDir()
	f := startFakeHost(t, filepath.Join(remoteHome, "sessions"), "cafe0001", "Server job", "/srv/proj",
		approvalLine(t, "ap1"))

	run := func(home string, args ...string) *exec.Cmd {
		c := exec.Command(bin, args...)
		c.Dir, c.Env = home, append(os.Environ(), "RUSH_HOME=.") // relative: see shortHome
		c.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
		return c
	}
	addr := "127.0.0.1:" + strconv.Itoa(freePort(t))
	serve := run(remoteHome, "serve", "--listen", addr)
	if err := serve.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { syscall.Kill(-serve.Process.Pid, syscall.SIGKILL); serve.Wait() })
	var tok string
	until(t, "serve's token", func() bool {
		b, _ := os.ReadFile(filepath.Join(remoteHome, "remote-token"))
		tok = strings.TrimSpace(string(b))
		return len(tok) == 64
	})
	until(t, "serve listening", func() bool {
		c, err := net.Dial("tcp", addr)
		if err == nil {
			c.Close()
		}
		return err == nil
	})
	if out, err := run(localHome, "remote", "add", "box", "http://"+addr, tok).CombinedOutput(); err != nil {
		t.Fatalf("remote add: %v %s", err, out)
	}
	attach := run(localHome, "remote", "attach", "box")
	if err := attach.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { syscall.Kill(-attach.Process.Pid, syscall.SIGKILL); attach.Wait() })

	t.Setenv("RUSH_HOME", ".") // this process is the local rush, in its folder
	old, _ := os.Getwd()
	if err := os.Chdir(localHome); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(old) })
	var id string
	until(t, "the remote session in the local list", func() bool {
		for _, i := range host.List() {
			if i.Remote == "box" {
				id = i.ID
				return host.Alive(i.HostPID)
			}
		}
		return false
	})
	info, _ := host.ReadInfo(id)
	if info.Cwd != filepath.Join(unreachable("box"), "/srv/proj") || info.Name != "Server job @box" || info.State != "idle" || !info.IsRemote() {
		t.Fatalf("listed as %+v", info)
	}

	c, err := host.Dial(id)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	var d host.Decoder
	var approval string
	for approval == "" {
		select {
		case l := <-c.Lines:
			evs, _ := d.Decode(l)
			for _, ev := range evs {
				if a, ok := ev.(event.Approval); ok {
					approval = a.ID
				}
			}
		case <-time.After(5 * time.Second):
			t.Fatal("the remote's approval didn't arrive")
		}
	}
	if err := c.Allow(approval, nil, true); err != nil {
		t.Fatal(err)
	}
	if got := f.op(t); got["op"] != "allow" || got["id"] != "ap1" || got["always"] != true {
		t.Fatalf("host got %v", got)
	}
	// an image crosses as its bytes, written on the remote: its local path never does
	pic := filepath.Join(t.TempDir(), "shot.png")
	if err := os.WriteFile(pic, []byte("\x89PNG fake"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := c.SendImages("look", []string{pic}, false); err != nil {
		t.Fatal(err)
	}
	got := f.op(t)
	imgs, _ := got["images"].([]any)
	if got["op"] != "send" || got["text"] != "look" || len(imgs) != 1 {
		t.Fatalf("host got %v", got)
	}
	p, _ := imgs[0].(string)
	if strings.Contains(p, pic) || !strings.HasSuffix(p, ".png") {
		t.Fatalf("host got image %q (the local path must not cross)", p)
	}
	if !filepath.IsAbs(p) { // serve's RUSH_HOME is relative, from its folder
		p = filepath.Join(remoteHome, p)
	}
	if b, err := os.ReadFile(p); err != nil || string(b) != "\x89PNG fake" {
		t.Fatalf("the image on the remote: %q %v", b, err)
	}
	if err := c.Interrupt(); err != nil {
		t.Fatal(err)
	}
	if f.op(t)["op"] != "interrupt" {
		t.Fatal("interrupt")
	}
	// not allowed remotely: the view is told, the host never hears of it
	if err := c.Retitle("/somewhere"); err != nil {
		t.Fatal(err)
	}
	for told := false; !told; {
		select {
		case l := <-c.Lines:
			evs, _ := d.Decode(l)
			for _, ev := range evs {
				if e, ok := ev.(host.ErrorEvent); ok && strings.Contains(e.Error, "isn't available") {
					told = true
				}
			}
		case <-time.After(5 * time.Second):
			t.Fatal("the view wasn't told")
		}
	}
	f.noOp(t)
	// a second attach to the same machine is refused
	if out, err := run(localHome, "remote", "attach", "box").CombinedOutput(); err == nil || !strings.Contains(string(out), "already attached") {
		t.Fatalf("second attach: %v %s", err, out)
	}
	// ending attach removes what it made
	syscall.Kill(attach.Process.Pid, syscall.SIGTERM)
	until(t, "its folders gone", func() bool { _, err := os.Stat(filepath.Join(localHome, "sessions", id)); return os.IsNotExist(err) })
	if _, err := os.Stat(filepath.Join(localHome, "remote-cwd", "box")); !os.IsNotExist(err) {
		t.Errorf("the unreachable folder was left: %v", err)
	}
}

// peerOf is a fake serve that lists the given remote session ids, all idle
// and alive, and streams nothing.
func peerOf(t *testing.T, ids ...string) Peer {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/sessions" {
			http.NotFound(w, r)
			return
		}
		var l []Session
		for _, id := range ids {
			l = append(l, Session{Info: host.Info{ID: id, Name: id, Cwd: "/srv/" + id, State: "idle", UpdatedAt: time.Now()}, Alive: true})
		}
		writeJSON(w, l)
	}))
	t.Cleanup(srv.Close)
	return Peer{Name: "box", URL: srv.URL, Token: "t"}
}

// runAttach runs Attach until the returned stop, which waits for it to end.
func runAttach(t *testing.T, p Peer) (stop func()) {
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- Attach(ctx, p, io.Discard) }()
	var once sync.Once
	stop = func() {
		once.Do(func() {
			cancel()
			select {
			case <-done:
			case <-time.After(10 * time.Second):
				t.Fatal("attach didn't end")
			}
		})
	}
	t.Cleanup(stop)
	return stop
}

func aliasOf(rid string) (host.Info, bool) {
	for _, i := range host.List() {
		if i.Remote == "box" && i.RemoteID == rid {
			return i, true
		}
	}
	return host.Info{}, false
}

func snapshot(t *testing.T, dir string) map[string]string {
	t.Helper()
	out := map[string]string{}
	err := filepath.WalkDir(dir, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.Type().IsRegular() {
			b, _ := os.ReadFile(p)
			out[p] = string(b)
		} else {
			out[p] = d.Type().String()
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return out
}

// An id that collides, with a session of this machine or a link, is not
// touched by attach, nor by it ending: the remote session takes another id,
// and everything of the existing one - config, info, a live socket - is what
// it was.
func TestAttachNeverTouchesACollidingSession(t *testing.T) {
	shortHome(t)
	root := host.Root()
	id0, id1 := localID("box", "r1", 0), localID("box", "r1", 1)

	// n=0: a real session folder, with a host answering on its socket
	d0 := filepath.Join(root, id0)
	if err := os.MkdirAll(d0, 0o700); err != nil {
		t.Fatal(err)
	}
	must0 := func(err error) {
		if err != nil {
			t.Fatal(err)
		}
	}
	must0(os.WriteFile(filepath.Join(d0, "config.json"), []byte(`{"id":"sentinel"}`), 0o600))
	must0(os.WriteFile(filepath.Join(d0, "info.json"), []byte(`{"id":"`+id0+`","name":"mine","hostPid":`+strconv.Itoa(os.Getpid())+`}`), 0o600))
	must0(os.WriteFile(filepath.Join(d0, "transcript"), []byte("precious"), 0o600))
	ln := listenIn(t, d0)
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			c.Close()
		}
	}()

	// n=1: a link to somewhere else that has things in it
	target := t.TempDir()
	must0(os.WriteFile(filepath.Join(target, "keep"), []byte("kept"), 0o600))
	must0(os.Symlink(target, filepath.Join(root, id1)))

	before0, beforeT := snapshot(t, d0), snapshot(t, target)
	stop := runAttach(t, peerOf(t, "r1"))
	var got host.Info
	until(t, "the remote session listed", func() bool { i, ok := aliasOf("r1"); got = i; return ok })
	if got.ID == id0 || got.ID == id1 {
		t.Fatalf("took %s, which was someone's", got.ID)
	}
	if got.ID != localID("box", "r1", 2) {
		t.Errorf("didn't take the next free id: %s", got.ID)
	}
	if c, err := net.Dial("unix", filepath.Join(d0, "host.sock")); err != nil {
		t.Fatalf("its socket was unlinked: %v", err)
	} else {
		c.Close()
	}
	stop()

	if after := snapshot(t, d0); !reflect.DeepEqual(after, before0) {
		t.Errorf("the session folder changed:\n%v\n%v", before0, after)
	}
	if after := snapshot(t, target); !reflect.DeepEqual(after, beforeT) {
		t.Errorf("the link's target changed:\n%v\n%v", beforeT, after)
	}
	if fi, err := os.Lstat(filepath.Join(root, id1)); err != nil || fi.Mode()&os.ModeSymlink == 0 {
		t.Errorf("the link is gone: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, got.ID)); !os.IsNotExist(err) {
		t.Errorf("its own folder was left: %v", err)
	}
	if c, err := net.Dial("unix", filepath.Join(d0, "host.sock")); err != nil {
		t.Errorf("its socket is gone after attach ended: %v", err)
	} else {
		c.Close()
	}
}

// purge takes back only what carries this machine's marker of an attach
// that has ended; a stale one of its own keeps its id.
func TestAttachPurgesOnlyWhatItMade(t *testing.T) {
	shortHome(t)
	root := host.Root()
	mk := func(name string, files map[string]string) string {
		d := filepath.Join(root, name)
		if err := os.MkdirAll(d, 0o700); err != nil {
			t.Fatal(err)
		}
		for f, c := range files {
			if err := os.WriteFile(filepath.Join(d, f), []byte(c), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		return d
	}
	dead := 0x7ffffff0 // no such process
	if host.Alive(dead) {
		t.Skip("pid in use")
	}
	m := func(machine, rid string, pid int) string {
		b, _ := jsonx.Marshal(marker{Machine: machine, RemoteID: rid, PID: pid, Token: "t"})
		return string(b)
	}
	// looks like an alias by what it says, but nothing proves it's attach's
	lookalike := mk("a0a0a0a0", map[string]string{"info.json": `{"id":"a0a0a0a0","remote":"box","remoteId":"x","hostPid":` + strconv.Itoa(dead) + `}`})
	// another machine's attach, ended
	other := mk("b0b0b0b0", map[string]string{markerName: m("elsewhere", "x", dead)})
	// this machine's attach, still running (this process)
	live := mk("c0c0c0c0", map[string]string{markerName: m("box", "x", os.Getpid())})
	// this machine's, ended, for r1: its own stale folder
	stale := filepath.Join(root, localID("box", "r1", 0))
	mk(filepath.Base(stale), map[string]string{markerName: m("box", "r1", dead), "info.json": "{}"})
	// this machine's, ended, for a session no longer there
	gone := mk("d0d0d0d0", map[string]string{markerName: m("box", "gone", dead)})

	runAttach(t, peerOf(t, "r1"))
	var got host.Info
	until(t, "r1 listed", func() bool { i, ok := aliasOf("r1"); got = i; return ok })
	for name, d := range map[string]string{"lookalike": lookalike, "other machine's": other, "a running attach's": live} {
		if _, err := os.Stat(d); err != nil {
			t.Errorf("%s was removed: %v", name, err)
		}
	}
	if _, err := os.Stat(gone); !os.IsNotExist(err) {
		t.Errorf("its own stale folder for a gone session was kept")
	}
	if got.ID != localID("box", "r1", 0) {
		t.Errorf("a stale folder of its own wasn't taken back under the same id: %s", got.ID)
	}
}

// If an alias's folder is replaced while attach runs, attach leaves the new
// one alone, now and at the end.
func TestAttachLeavesAReplacedFolder(t *testing.T) {
	shortHome(t)
	stop := runAttach(t, peerOf(t, "r1"))
	var got host.Info
	until(t, "listed", func() bool { i, ok := aliasOf("r1"); got = i; return ok })
	dir := filepath.Join(host.Root(), got.ID)
	if err := os.Remove(filepath.Join(dir, markerName)); err != nil { // as if someone else now owned it
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "theirs"), []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	stop()
	if b, err := os.ReadFile(filepath.Join(dir, "theirs")); err != nil || string(b) != "x" {
		t.Fatalf("removed a folder it no longer owned: %v", err)
	}
}
