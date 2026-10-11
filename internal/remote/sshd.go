package remote

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/creack/pty"
	"golang.org/x/crypto/ssh"

	"github.com/0xdeafcafe/rush/internal/state"
)

// SSH: rush itself, its whole TUI, from any SSH client on your tailnet
// (Termius, Blink, a laptop's ssh). Each paired key is a device; a key
// that isn't one yet is asked for a pairing code (Settings › Remote ›
// Pair a device), which pairs it. Each device has one rush, kept for
// sshKeep after its connection drops: connect again and it's where you
// left it, as tmux would keep it, with no tmux.

const sshKeep = 30 * time.Minute

// sshExe is the rush each device runs; tests set their own.
var sshExe = os.Executable

func sshHostKey() (ssh.Signer, error) {
	path := filepath.Join(state.Dir(), "remote-ssh-host-key")
	if b, err := os.ReadFile(path); err == nil {
		return ssh.ParsePrivateKey(b)
	}
	_, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	blk, err := ssh.MarshalPrivateKey(priv, "rush serve")
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(state.Dir(), 0o700); err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, pem.EncodeToMemory(blk), 0o600); err != nil {
		return nil, err
	}
	return ssh.NewSignerFromKey(priv)
}

// SSHFingerprint is serve's SSH host key's, for checking the first connection.
func SSHFingerprint() string {
	k, err := sshHostKey()
	if err != nil {
		return ""
	}
	return ssh.FingerprintSHA256(k.PublicKey())
}

// serveSSH serves SSH on cfg's address until ctx ends, then ends every
// device's rush.
func (s *Server) serveSSH(ctx context.Context, log io.Writer) error {
	key, err := sshHostKey()
	if err != nil {
		return err
	}
	conf := &ssh.ServerConfig{PublicKeyCallback: s.sshKey, ServerVersion: "SSH-2.0-rush"}
	conf.AddHostKey(key)
	ln, err := net.Listen("tcp", s.Config.SSHListenOr())
	if err != nil {
		return err
	}
	fmt.Fprintf(log, "rush serve: ssh on %s\n", ln.Addr())
	go func() { <-ctx.Done(); ln.Close() }()
	defer s.tuis.endAll()
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return err
		}
		go s.sshConn(c, conf)
	}
}

// sshKey lets in a paired key; any other key, proven to be the client's,
// is asked for a pairing code, which pairs it.
func (s *Server) sshKey(meta ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
	line := strings.TrimSpace(string(ssh.MarshalAuthorizedKey(key)))
	if dev := s.devs.ssh(line); dev != nil {
		return &ssh.Permissions{Extensions: map[string]string{"device": dev.ID}}, nil
	}
	ask := func(meta ssh.ConnMetadata, ch ssh.KeyboardInteractiveChallenge) (*ssh.Permissions, error) {
		ans, err := ch("", "This key isn't paired with rush on "+s.Config.Name+" yet.\n"+
			"In rush there: Settings › Remote › Pair a device shows a code.", []string{"Pairing code: "}, []bool{true})
		if err != nil || len(ans) != 1 || !s.pairs.take(ans[0]) {
			time.Sleep(time.Second)
			return nil, errors.New("not a pairing code")
		}
		fp := ssh.FingerprintSHA256(key)
		dev, err := s.devs.add(Device{Kind: "ssh", Name: sshClientName(string(meta.ClientVersion())) + " · " + fp[7:15], Key: line, Fingerprint: fp})
		if err != nil {
			return nil, err
		}
		return &ssh.Permissions{Extensions: map[string]string{"device": dev.ID}}, nil
	}
	return nil, &ssh.PartialSuccessError{Next: ssh.ServerAuthCallbacks{KeyboardInteractiveCallback: ask}}
}

// sshClientName is a client's name from its version, "SSH-2.0-OpenSSH_9.8"
// being OpenSSH.
func sshClientName(v string) string {
	v = strings.TrimPrefix(v, "SSH-2.0-")
	if i := strings.IndexAny(v, "_- /"); i > 0 {
		v = v[:i]
	}
	return or(v, "SSH")
}

func (s *Server) sshConn(c net.Conn, conf *ssh.ServerConfig) {
	sc, chans, reqs, err := ssh.NewServerConn(c, conf)
	if err != nil {
		c.Close()
		return
	}
	id := sc.Permissions.Extensions["device"]
	defer s.live.add(id, func() { sc.Close(); s.tuis.end(id) })()
	defer s.devs.touch(id) // seen until now
	go ssh.DiscardRequests(reqs)
	for nc := range chans {
		if nc.ChannelType() != "session" {
			_ = nc.Reject(ssh.UnknownChannelType, "only a session")
			continue
		}
		ch, creqs, err := nc.Accept()
		if err != nil {
			continue
		}
		go s.sshSession(id, ch, creqs)
	}
}

// sshSession is one SSH session: its terminal shows the device's rush.
func (s *Server) sshSession(id string, ch ssh.Channel, reqs <-chan *ssh.Request) {
	size := &pty.Winsize{Cols: 80, Rows: 24}
	term := "xterm-256color"
	var t *tui
	for req := range reqs {
		ok := false
		switch req.Type {
		case "pty-req":
			var p struct {
				Term       string
				Cols, Rows uint32
				W, H       uint32
				Modes      string
			}
			if ssh.Unmarshal(req.Payload, &p) == nil {
				term, size = or(p.Term, term), &pty.Winsize{Cols: uint16(p.Cols), Rows: uint16(p.Rows)}
				ok = true
			}
		case "window-change":
			if len(req.Payload) >= 8 {
				size = &pty.Winsize{Cols: uint16(binary.BigEndian.Uint32(req.Payload)), Rows: uint16(binary.BigEndian.Uint32(req.Payload[4:]))}
				if t != nil {
					t.resize(size)
				}
			}
		case "env":
			ok = true // taken, not used
		case "shell":
			if t == nil {
				var err error
				if t, err = s.tuis.attach(id, ch, term, size); err != nil {
					fmt.Fprintf(ch.Stderr(), "rush didn't start: %v\r\n", err)
					exitWith(ch, 1)
					return
				}
				ok = true
			}
		case "exec":
			fmt.Fprintf(ch.Stderr(), "rush over SSH is rush itself: ssh without a command.\r\n")
			exitWith(ch, 1)
			return
		}
		if req.WantReply {
			_ = req.Reply(ok, nil)
		}
	}
	if t != nil {
		t.detach(ch)
	}
	ch.Close()
}

func exitWith(ch ssh.Channel, code uint32) {
	_, _ = ch.SendRequest("exit-status", false, ssh.Marshal(struct{ C uint32 }{code}))
	ch.Close()
}

// tuis is each device's rush, by device.
type tuis struct {
	mu sync.Mutex
	by map[string]*tui
}

// tui is one device's rush on a pty, shown on its newest connection.
type tui struct {
	cmd  *exec.Cmd
	ptmx *os.File

	mu    sync.Mutex
	on    ssh.Channel // the connection it shows on, or nil between them
	modes map[string]byte
	tail  []byte
	keep  *time.Timer
	ended bool
}

func (ts *tuis) attach(id string, ch ssh.Channel, term string, size *pty.Winsize) (*tui, error) {
	ts.mu.Lock()
	defer ts.mu.Unlock()
	if t := ts.by[id]; t != nil {
		t.show(ch, size)
		return t, nil
	}
	exe, err := sshExe()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(exe)
	cmd.Dir, _ = os.UserHomeDir()
	cmd.Env = append(os.Environ(), "TERM="+term, "COLORTERM=truecolor", "RUSH_VIA=ssh")
	if os.Getenv("LANG") == "" {
		cmd.Env = append(cmd.Env, "LANG=en_US.UTF-8")
	}
	ptmx, err := pty.StartWithSize(cmd, size)
	if err != nil {
		return nil, err
	}
	t := &tui{cmd: cmd, ptmx: ptmx, on: ch, modes: map[string]byte{}}
	if ts.by == nil {
		ts.by = map[string]*tui{}
	}
	ts.by[id] = t
	go func() {
		t.pump()
		_ = cmd.Wait()
		ts.mu.Lock()
		if ts.by[id] == t {
			delete(ts.by, id)
		}
		ts.mu.Unlock()
		t.mu.Lock()
		t.ended = true
		if t.keep != nil {
			t.keep.Stop()
		}
		if t.on != nil {
			exitWith(t.on, 0)
		}
		t.mu.Unlock()
	}()
	go func() { _, _ = io.Copy(ptmx, chReader{t, ch}) }()
	return t, nil
}

// chReader is a connection's keys, read while it's the one rush shows on.
type chReader struct {
	t  *tui
	ch ssh.Channel
}

func (r chReader) Read(p []byte) (int, error) {
	n, err := r.ch.Read(p)
	r.t.mu.Lock()
	defer r.t.mu.Unlock()
	if r.t.on != r.ch {
		return 0, io.EOF
	}
	return n, err
}

// decMode is a terminal mode rush sets (the alternate screen, the mouse,
// pasting): a new connection's terminal is told them again.
var decMode = regexp.MustCompile(`\x1b\[\?([\d;]+)([hl])`)

// pump copies rush's screen to the connection showing it, noting the
// modes it sets on the way; between connections it's read and dropped,
// and the next gets a whole new frame.
func (t *tui) pump() {
	buf := make([]byte, 32<<10)
	for {
		n, err := t.ptmx.Read(buf)
		if n > 0 {
			t.mu.Lock()
			scan := append(t.tail, buf[:n]...)
			for _, m := range decMode.FindAllSubmatch(scan, -1) {
				for _, mode := range strings.Split(string(m[1]), ";") {
					t.modes[mode] = m[2][0]
				}
			}
			t.tail = append([]byte(nil), scan[max(0, len(scan)-16):]...)
			if t.on != nil {
				_, _ = t.on.Write(buf[:n])
			}
			t.mu.Unlock()
		}
		if err != nil {
			return
		}
	}
}

// show moves rush onto ch, from the connection it was on if any.
func (t *tui) show(ch ssh.Channel, size *pty.Winsize) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.keep != nil {
		t.keep.Stop()
		t.keep = nil
	}
	if old := t.on; old != nil && old != ch {
		fmt.Fprint(old, "\x1b[?1049l\r\nrush moved to another connection.\r\n")
		exitWith(old, 0)
	}
	t.on = ch
	var set strings.Builder
	for mode, hl := range t.modes {
		set.WriteString("\x1b[?" + mode + string(hl))
	}
	_, _ = ch.Write([]byte(set.String()))
	go func() { _, _ = io.Copy(t.ptmx, chReader{t, ch}) }()
	_ = pty.Setsize(t.ptmx, size)
	// The same size wouldn't redraw on its own; a resize has rush draw it all.
	_ = t.cmd.Process.Signal(syscall.SIGWINCH)
}

func (t *tui) resize(size *pty.Winsize) { _ = pty.Setsize(t.ptmx, size) }

// detach is ch's connection gone: rush is kept for sshKeep for the next.
func (t *tui) detach(ch ssh.Channel) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.on != ch || t.ended {
		return
	}
	t.on = nil
	t.keep = time.AfterFunc(sshKeep, t.stop)
}

func (t *tui) stop() { _ = t.cmd.Process.Signal(syscall.SIGHUP) }

func (ts *tuis) end(id string) {
	ts.mu.Lock()
	t := ts.by[id]
	ts.mu.Unlock()
	if t != nil {
		t.stop()
	}
}

func (ts *tuis) endAll() {
	ts.mu.Lock()
	defer ts.mu.Unlock()
	for _, t := range ts.by {
		t.stop()
	}
}
