// Package daemon speaks the Claude Code daemon's control socket (proto 1):
// one JSON request line, one JSON response line. After a successful attach
// the connection becomes a raw terminal stream in both directions.
package daemon

import (
	"bufio"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
)

const proto = 1

type Client struct {
	Account claude.Account
}

// SockDir is /tmp/cc-daemon-<uid>/<sha256(configDir)[:8]>.
func (c Client) SockDir() string {
	sum := sha256.Sum256([]byte(c.Account.ConfigDir))
	return filepath.Join("/tmp", fmt.Sprintf("cc-daemon-%d", os.Getuid()), hex.EncodeToString(sum[:])[:8])
}

func (c Client) controlSock() string { return filepath.Join(c.SockDir(), "control.sock") }

// Running reports whether this account's daemon is up.
func (c Client) Running() bool {
	_, err := os.Stat(c.controlSock())
	return err == nil
}

func (c Client) key() string {
	b, err := os.ReadFile(filepath.Join(c.Account.ConfigDir, "daemon", "control.key"))
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

// Error is a refusal from the daemon, e.g. ENOJOB or EKICKED.
type Error struct {
	Code, Message string
}

func (e *Error) Error() string { return e.Code + ": " + e.Message }

type response struct {
	OK      bool   `json:"ok"`
	Code    string `json:"code"`
	Error   string `json:"error"`
	Message string `json:"message"`
}

func (c Client) dial(req map[string]any) (net.Conn, *bufio.Reader, []byte, error) {
	conn, err := net.DialTimeout("unix", c.controlSock(), 2*time.Second)
	if err != nil {
		return nil, nil, nil, err
	}
	req["proto"] = proto
	if k := c.key(); k != "" {
		req["auth"] = k
	}
	b, _ := jsonx.Marshal(req)
	_ = conn.SetDeadline(time.Now().Add(10 * time.Second))
	if _, err := conn.Write(append(b, '\n')); err != nil {
		conn.Close()
		return nil, nil, nil, err
	}
	r := bufio.NewReaderSize(conn, 64<<10)
	line, err := r.ReadBytes('\n')
	if err != nil {
		conn.Close()
		return nil, nil, nil, err
	}
	var res response
	if err := jsonx.Unmarshal(line, &res); err != nil {
		conn.Close()
		return nil, nil, nil, err
	}
	if !res.OK {
		conn.Close()
		msg := res.Message
		if msg == "" {
			msg = res.Error
		}
		code := res.Code
		if code == "" {
			code = "EFAIL"
		}
		return nil, nil, nil, &Error{Code: code, Message: msg}
	}
	_ = conn.SetDeadline(time.Time{})
	return conn, r, line, nil
}

func (c Client) call(req map[string]any) error {
	conn, _, _, err := c.dial(req)
	if err != nil {
		return err
	}
	return conn.Close()
}

// Reply types text into a session as if the user sent it.
func (c Client) Reply(short, text string) error {
	return c.call(map[string]any{"op": "reply", "short": short, "text": text})
}

// Kill asks the daemon to stop a session; the conversation is kept.
func (c Client) Kill(short string) error {
	return c.call(map[string]any{"op": "kill", "short": short})
}

func (c Client) Resize(short, attachID string, cols, rows int) error {
	return c.call(map[string]any{"op": "resize", "short": short, "attachId": attachID, "cols": cols, "rows": rows})
}

type AttachInfo struct {
	DecModes []int  `json:"decModes"`
	Via      string `json:"via"`
	State    string `json:"state"`
	Stale    bool   `json:"stale"`
}

// Attach opens the live terminal stream of a session.
func (c Client) Attach(short, attachID string, cols, rows int) (net.Conn, *bufio.Reader, AttachInfo, error) {
	var info AttachInfo
	conn, r, line, err := c.dial(map[string]any{
		"op": "attach", "short": short, "attachId": attachID, "cols": cols, "rows": rows,
	})
	if err != nil {
		return nil, nil, info, err
	}
	_ = jsonx.Unmarshal(line, &info)
	return conn, r, info, nil
}

func IsRefusal(err error, code string) bool {
	var e *Error
	return errors.As(err, &e) && e.Code == code
}
