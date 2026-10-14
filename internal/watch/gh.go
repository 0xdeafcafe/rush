// Package watch waits on things for agents, so they don't poll: rush asks
// on their behalf, cheaply, and says once what happened. A pull request's
// checks are asked of GitHub through the host's gh, signed in as the user.
// It's files and flock under state.Dir()/watch, no daemon: every agent
// watching the same pull request shares one answer, and asking again
// sends the last ETag, so an answer that hasn't changed (304) costs none of
// the user's rate limit.
package watch

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Root is where watches keep what they share.
func Root() string { return filepath.Join(state.Dir(), "watch") }

// cached is one gh api answer as kept for every watcher.
type cached struct {
	ETag      string    `json:"etag"`
	Body      []byte    `json:"body"`
	At        time.Time `json:"at"`
	Remaining int       `json:"remaining"` // the rate limit left when it was asked
}

// Runner runs gh api with args and gives its output, headers first (-i).
// Tests set it.
var Runner = func(ctx context.Context, args ...string) ([]byte, []byte, error) {
	cmd := exec.CommandContext(ctx, "gh", append([]string{"api", "-i"}, args...)...)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	err := cmd.Run()
	return out.Bytes(), errb.Bytes(), err
}

// errLimited is GitHub saying the rate limit is spent.
var errLimited = errors.New("GitHub's rate limit is spent")

// Get is path's body from GitHub. An answer any watcher had within fresh
// is used as it is; an older one is asked again with its ETag, and kept
// when GitHub says it hasn't changed. remaining is the rate limit left,
// -1 when unknown.
func Get(ctx context.Context, path string, fresh time.Duration) (body []byte, remaining int, err error) {
	dir := filepath.Join(Root(), "gh")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, -1, err
	}
	sum := sha256.Sum256([]byte(path))
	file := filepath.Join(dir, hex.EncodeToString(sum[:8]))
	lock, err := os.OpenFile(file+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, -1, err
	}
	defer lock.Close()
	// One asks while the rest wait for its answer.
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX); err != nil {
		return nil, -1, err
	}
	defer func() { _ = syscall.Flock(int(lock.Fd()), syscall.LOCK_UN) }()

	var c cached
	if b, err := os.ReadFile(file + ".json"); err == nil {
		_ = jsonx.Unmarshal(b, &c)
	}
	if c.Body != nil && time.Since(c.At) < fresh {
		return c.Body, c.Remaining, nil
	}
	args := []string{path}
	if c.ETag != "" && c.Body != nil {
		args = []string{"-H", "If-None-Match: " + c.ETag, path}
	}
	out, stderr, runErr := Runner(ctx, args...)
	status, h, b := parseResponse(out)
	switch {
	case status == 304 && c.Body != nil:
		c.At = time.Now()
		if n, ok := h["x-ratelimit-remaining"]; ok {
			c.Remaining, _ = strconv.Atoi(n)
		}
	case status == 200:
		c = cached{ETag: h["etag"], Body: b, At: time.Now(), Remaining: -1}
		if n, ok := h["x-ratelimit-remaining"]; ok {
			c.Remaining, _ = strconv.Atoi(n)
		}
	case status == 403 || status == 429:
		if h["x-ratelimit-remaining"] == "0" || status == 429 {
			return nil, 0, errLimited
		}
		return nil, -1, ghError(status, b, stderr, runErr)
	default:
		return nil, -1, ghError(status, b, stderr, runErr)
	}
	if enc, err := jsonx.Marshal(c); err == nil {
		tmp := file + ".tmp"
		if os.WriteFile(tmp, enc, 0o600) == nil {
			_ = os.Rename(tmp, file+".json")
		}
	}
	return c.Body, c.Remaining, nil
}

// Raw is path's body from GitHub, unshared and uncached: a job's log.
func Raw(ctx context.Context, path string) ([]byte, error) {
	// A log is coloured, which gh won't print unless asked; an older gh
	// doesn't know the flag, and prints it anyway.
	out, stderr, err := Runner(ctx, "--allow-escape-sequences", path)
	if bytes.Contains(stderr, []byte("unknown flag")) {
		out, stderr, err = Runner(ctx, path)
	}
	status, _, b := parseResponse(out)
	if status != 200 {
		return nil, ghError(status, b, stderr, err)
	}
	return b, nil
}

// parseResponse splits gh api -i's output into its status, its headers
// (lowercased; the last response's, past any redirect) and its body.
func parseResponse(out []byte) (int, map[string]string, []byte) {
	h := map[string]string{}
	status := 0
	for bytes.HasPrefix(out, []byte("HTTP/")) {
		head, body, ok := bytes.Cut(out, []byte("\r\n\r\n"))
		if !ok {
			head, body, _ = bytes.Cut(out, []byte("\n\n"))
		}
		clear(h)
		sc := bufio.NewScanner(bytes.NewReader(head))
		for first := true; sc.Scan(); first = false {
			line := strings.TrimRight(sc.Text(), "\r")
			if first {
				if f := strings.Fields(line); len(f) > 1 {
					status, _ = strconv.Atoi(f[1])
				}
				continue
			}
			if k, v, ok := strings.Cut(line, ":"); ok {
				h[strings.ToLower(strings.TrimSpace(k))] = strings.TrimSpace(v)
			}
		}
		out = body
	}
	return status, h, out
}

// ghError is why a gh api call failed, in a few words.
func ghError(status int, body, stderr []byte, err error) error {
	if errors.Is(err, exec.ErrNotFound) {
		return errors.New("gh isn't installed")
	}
	var m struct {
		Message string `json:"message"`
	}
	if jsonx.Unmarshal(body, &m) == nil && m.Message != "" {
		return errors.New("GitHub says: " + m.Message + " (HTTP " + strconv.Itoa(status) + ")")
	}
	msg := strings.TrimSpace(string(stderr))
	switch {
	case strings.Contains(msg, "gh auth login") || strings.Contains(msg, "authentication"):
		return errors.New("gh isn't signed in · gh auth login")
	case msg != "":
		if i := strings.IndexByte(msg, '\n'); i > 0 {
			msg = msg[:i]
		}
		return errors.New(msg)
	case err != nil:
		return err
	}
	return errors.New("GitHub answered HTTP " + strconv.Itoa(status))
}
