package remote

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

// lockedBuf is a session's screen as it arrives.
type lockedBuf struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (l *lockedBuf) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.Write(p)
}

func (l *lockedBuf) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.String()
}

// A key that isn't paired is asked for a code, and the code pairs it; it
// then gets in with no code, to the same rush it left (which is told its
// screen modes again and redraws), and revoking it ends that rush.
func TestSSHPairsKeysAndKeepsTheirRush(t *testing.T) {
	s := newServer(t)
	dir := t.TempDir()
	exe := filepath.Join(dir, "rush")
	if err := os.WriteFile(exe, []byte("#!/bin/sh\nprintf '\\033[?1049h'\necho \"ready $$\"\ntrap 'echo redraw' WINCH\nwhile :; do read l && echo \"got $l\"; done\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	sshExe = func() (string, error) { return exe, nil }
	t.Cleanup(func() { sshExe = os.Executable })
	s.Config.SSHListen = "127.0.0.1:0"
	key, err := sshHostKey()
	if err != nil {
		t.Fatal(err)
	}
	conf := &ssh.ServerConfig{PublicKeyCallback: s.sshKey}
	conf.AddHostKey(key)
	// serveSSH's loop, on a port of the test's own.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go s.sshConn(c, conf)
		}
	}()
	t.Cleanup(func() { ln.Close(); s.tuis.endAll() })

	_, priv, _ := ed25519.GenerateKey(rand.Reader)
	signer, _ := ssh.NewSignerFromKey(priv)
	dial := func(code string) (*ssh.Client, error) {
		auth := []ssh.AuthMethod{ssh.PublicKeys(signer)}
		if code != "" {
			auth = append(auth, ssh.KeyboardInteractive(func(_, _ string, q []string, _ []bool) ([]string, error) {
				return []string{code}, nil
			}))
		}
		return ssh.Dial("tcp", ln.Addr().String(), &ssh.ClientConfig{User: "me", Auth: auth,
			HostKeyCallback: ssh.FixedHostKey(key.PublicKey()), ClientVersion: "SSH-2.0-Termius_7.1", Timeout: 5 * time.Second})
	}
	open := func(c *ssh.Client) (io.Writer, *lockedBuf) {
		sess, err := c.NewSession()
		if err != nil {
			t.Fatal(err)
		}
		var out lockedBuf
		sess.Stdout = &out
		in, _ := sess.StdinPipe()
		if err := sess.RequestPty("xterm-256color", 40, 100, ssh.TerminalModes{}); err != nil {
			t.Fatal(err)
		}
		if err := sess.Shell(); err != nil {
			t.Fatal(err)
		}
		return in, &out
	}
	shows := func(out *lockedBuf, want string) {
		t.Helper()
		until(t, "the screen to show "+want, func() bool { return strings.Contains(out.String(), want) })
	}

	if _, err := dial(""); err == nil {
		t.Fatal("an unpaired key got in")
	}
	if _, err := dial("WRONGCOD"); err == nil {
		t.Fatal("a wrong code got in")
	}
	code, _ := s.pairs.issue()
	c, err := dial(strings.ToLower(PairCode(code)))
	if err != nil {
		t.Fatalf("pairing: %v", err)
	}
	in, out := open(c)
	shows(out, "ready ")
	pid := strings.Fields(out.String()[strings.Index(out.String(), "ready "):])[1]
	io.WriteString(in, "one\r")
	shows(out, "got one")
	devs := s.devs.all()
	if len(devs) != 1 || devs[0].Kind != "ssh" || !strings.HasPrefix(devs[0].Name, "Termius · ") {
		t.Fatalf("devices: %+v", devs)
	}
	c.Close()
	until(t, "the connection to go", func() bool { return s.live.count(devs[0].ID) == 0 })

	c, err = dial("")
	if err != nil {
		t.Fatalf("a paired key: %v", err)
	}
	in, out = open(c)
	shows(out, "\x1b[?1049h")
	shows(out, "redraw")
	io.WriteString(in, "two\r")
	shows(out, "got two")
	if strings.Contains(out.String(), "ready") {
		t.Errorf("a new rush, not the one kept (was %s): %q", pid, out.String())
	}

	if w := do(t, s.Handler(), "POST", "/api/devices/"+devs[0].ID+"/revoke", "{}", map[string]string{"Authorization": "Bearer " + s.Token, "Content-Type": "application/json"}); w.Code != 200 {
		t.Fatalf("revoke: %d", w.Code)
	}
	until(t, "its rush to end", func() bool {
		s.tuis.mu.Lock()
		defer s.tuis.mu.Unlock()
		return len(s.tuis.by) == 0
	})
	if _, err := dial(""); err == nil {
		t.Error("a revoked key got in")
	}
}

// Ten wrong codes void the live ones: a short code can't be guessed at.
func TestPairCodesStopGuessing(t *testing.T) {
	var p pairCodes
	code, _ := p.issue()
	for range pairMisses {
		p.take("AAAAAAAA")
	}
	if p.take(code) {
		t.Error("a code still worked after ten wrong ones")
	}
}
