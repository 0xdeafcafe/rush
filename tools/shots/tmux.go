package main

import (
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// term is rush running in a tmux server of its own, out of sight, at a
// terminal size of the shot's choosing.
type term struct {
	sock string
}

func (t *term) tmux(args ...string) (string, error) {
	out, err := exec.Command("tmux", append([]string{"-L", t.sock, "-f", "/dev/null"}, args...)...).CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("tmux %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return string(out), nil
}

// start runs the program in a new w×h terminal.
func start(sock, program string, w, h int) (*term, error) {
	t := &term{sock: sock}
	_, _ = t.tmux("kill-server")
	// The trailing sleep keeps the pane, and what rush said, if it exits.
	_, err := t.tmux("new-session", "-d", "-s", "s", "-x", fmt.Sprint(w), "-y", fmt.Sprint(h), program+"; sleep 60")
	return t, err
}

func (t *term) stop() { _, _ = t.tmux("kill-server") }

// screen is what the terminal shows, as text, with its colours when ansi.
func (t *term) screen(ansi bool) string {
	args := []string{"capture-pane", "-p", "-t", "s"}
	if ansi {
		args = append(args, "-e")
	}
	out, _ := t.tmux(args...)
	return out
}

// until waits for the screen to show text.
func (t *term) until(text string, limit time.Duration) error {
	end := time.Now().Add(limit)
	for time.Now().Before(end) {
		if strings.Contains(t.screen(false), text) {
			return nil
		}
		time.Sleep(100 * time.Millisecond)
	}
	return fmt.Errorf("the screen never showed %q:\n%s", text, t.screen(false))
}

// do runs one of a shot's steps:
//
//	text=…    types it
//	wait=2s   waits that long
//	until=…   waits for the screen to show it (10s at most)
//	anything else is a key, as tmux names it (Enter, Up, C-k, ])
//
// Every key is followed by a short pause, as a person's would be, so
// rush has drawn what it led to before the next.
func (t *term) do(s string) error {
	switch {
	case strings.HasPrefix(s, "text="):
		_, err := t.tmux("send-keys", "-t", "s", "-l", strings.TrimPrefix(s, "text="))
		time.Sleep(400 * time.Millisecond)
		return err
	case strings.HasPrefix(s, "wait="):
		d, err := time.ParseDuration(strings.TrimPrefix(s, "wait="))
		if err != nil {
			return err
		}
		time.Sleep(d)
		return nil
	case strings.HasPrefix(s, "until="):
		return t.until(strings.TrimPrefix(s, "until="), 10*time.Second)
	}
	_, err := t.tmux("send-keys", "-t", "s", s)
	time.Sleep(300 * time.Millisecond)
	return err
}
