package claude

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/daemon"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// Claude Code's own ways to change a session outside rush mode: its
// background daemon's control socket first, the claude CLI as the fallback.

var (
	shortID  = regexp.MustCompile(`\b[0-9a-f]{8}\b`)
	attachID = regexp.MustCompile(`claude attach ([0-9a-f]{8})`)
)

// newID reads the session id claude --bg prints in its "claude attach <id>" hint.
func newID(out string) string {
	if m := attachID.FindStringSubmatch(out); m != nil {
		return m[1]
	}
	return shortID.FindString(out)
}

func claudeCmd(acct claude.Account, dir string, args ...string) *exec.Cmd {
	c := exec.Command(claude.Program, args...)
	c.Env = acct.Env()
	c.Dir = dir
	return c
}

func run(c *exec.Cmd) (string, error) {
	var out bytes.Buffer
	c.Stdout, c.Stderr = &out, &out
	err := c.Run()
	s := strings.TrimSpace(out.String())
	if err != nil {
		if s == "" {
			s = err.Error()
		}
		return s, fmt.Errorf("%s", lastLine(s))
	}
	return s, nil
}

func lastLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.LastIndexByte(s, '\n'); i >= 0 {
		return s[i+1:]
	}
	return s
}

// Dispatch starts a new background session and returns its short id.
func (Adapter) Dispatch(p agent.Profile, dir, prompt string, flags ...string) (string, error) {
	args := append([]string{"--bg"}, flags...)
	args = append(args, "--", prompt) // a prompt starting with - is still a prompt
	out, err := run(claudeCmd(claude.AccountOf(p), dir, args...))
	if err != nil {
		return "", err
	}
	return newID(out), nil
}

// Stop ends a session's process and keeps its conversation. When the daemon
// and the CLI both fail, or the process outlives them, pid gets a SIGTERM.
func (Adapter) Stop(p agent.Profile, short string, pid int) error {
	return stop(claude.AccountOf(p), short, pid)
}

func stop(acct claude.Account, short string, pid int) error {
	err := (daemon.Client{Account: acct}).Kill(short)
	if err != nil {
		_, err = run(claudeCmd(acct, "", "stop", short))
	}
	if pid == 0 || waitExit(pid, 5*time.Second) {
		return err
	}
	return proc.Kill(pid, syscall.SIGTERM)
}

func waitExit(pid int, max time.Duration) bool {
	deadline := time.Now().Add(max)
	for time.Now().Before(deadline) {
		if syscall.Kill(pid, 0) != nil {
			return true
		}
		time.Sleep(200 * time.Millisecond)
	}
	return false
}

// Remove deletes a session, and its worktree when Claude Code deems it safe.
func (Adapter) Remove(p agent.Profile, short string) error {
	_, err := run(claudeCmd(claude.AccountOf(p), "", "rm", short))
	return err
}

// Reply sends a message to a background session through the daemon. One
// the daemon has let go of (ENOJOB) is agent.ErrGone.
func (Adapter) Reply(p agent.Profile, short, text string) error {
	c := daemon.Client{Account: claude.AccountOf(p)}
	if !c.Running() {
		return errors.New("Claude Code's background agents service isn't running, so it can't take a message; open the agent with enter instead")
	}
	err := c.Reply(short, text)
	if daemon.IsRefusal(err, "ENOJOB") {
		return fmt.Errorf("%w: %w", agent.ErrGone, err)
	}
	return err
}

// Attach is the claude CLI's own attach, used when the socket refuses.
func (Adapter) Attach(p agent.Profile, short string) *exec.Cmd {
	return claudeCmd(claude.AccountOf(p), "", "attach", short)
}

// Login signs the config folder p is in to an account.
func (Adapter) Login(p agent.Profile) *exec.Cmd {
	return claudeCmd(claude.AccountOf(p), "", "auth", "login")
}

// Screen opens Claude Code's own screen for a slash command (/plugin,
// /hooks, …) in dir. Nothing is sent to a model, and no conversation is
// kept: it's a fresh Claude Code that starts on the command. hint is
// printed above it (how to get back).
func (Adapter) Screen(p agent.Profile, dir, command, hint string) *exec.Cmd {
	c := exec.Command("sh", "-c", `printf '\033[2J\033[H%s\n\n' "$1"; exec claude "/$2"`, "sh", hint, command)
	c.Env = claude.AccountOf(p).Env()
	c.Dir = dir
	return c
}

// Move moves a conversation: stop it, make its transcript visible to the
// target account and folder, and resume it there with its original flags
// (from its job file, the listing's Extra).
func (Adapter) Move(mv *agent.Move) (string, error) {
	j, _ := mv.Extra.(claude.Job)
	j.Job = mv.Job
	from, to := claude.AccountOf(mv.From), claude.AccountOf(mv.To)
	if j.SessionID == "" || j.TranscriptPath == "" {
		return "", errors.New("this agent has no saved conversation to move")
	}
	dir := mv.Dir
	if dir == "" {
		dir = j.Cwd
	}
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		return "", fmt.Errorf("folder not found: %s", dir)
	}
	if _, running := claude.ReadRoster(from).Workers[j.ID]; running {
		if err := stop(from, j.ID, 0); err != nil {
			return "", fmt.Errorf("could not stop the agent first: %w", err)
		}
		if !waitStopped(from, j.ID, 10*time.Second) {
			return "", errors.New("the agent is still stopping; try again in a moment")
		}
	}
	dst := filepath.Join(to.ProjectsDir(), claude.ProjectSlug(dir), j.SessionID+".jsonl")
	if dst != j.TranscriptPath {
		if err := copyFile(j.TranscriptPath, dst); err != nil {
			return "", fmt.Errorf("could not copy the conversation: %w", err)
		}
	}
	args := []string{"--bg", "--resume", j.SessionID}
	args = append(args, respawnFlags(j.RespawnFlags)...)
	for _, d := range mv.AddDirs {
		args = append(args, "--add-dir", d)
	}
	if mv.Note != "" {
		args = append(args, "--", mv.Note) // --add-dir takes many values; end them first
	}
	out, err := run(claudeCmd(to, dir, args...))
	if err != nil {
		return "", err
	}
	// Resuming in the background forks the conversation into a new session.
	m := attachID.FindStringSubmatch(out)
	if m == nil {
		return "", errors.New("resumed, but Claude Code didn't say the new session's id; it will appear in the list")
	}
	return m[1], nil
}

// respawnFlags keeps the flags that describe the agent, not the old session.
func respawnFlags(flags []string) []string {
	var out []string
	for i := 0; i < len(flags); i++ {
		f := flags[i]
		switch {
		case f == "--resume" || f == "--session-id" || f == "-r":
			i++
			continue
		case f == "--fork-session" || f == "-c" || f == "--continue",
			strings.HasPrefix(f, "--resume=") || strings.HasPrefix(f, "--session-id="):
			continue
		}
		out = append(out, flags[i])
	}
	return out
}

func waitStopped(acct claude.Account, short string, max time.Duration) bool {
	deadline := time.Now().Add(max)
	for time.Now().Before(deadline) {
		if _, ok := claude.ReadRoster(acct).Workers[short]; !ok {
			return true
		}
		time.Sleep(200 * time.Millisecond)
	}
	return false
}

func copyFile(src, dst string) error {
	if err := os.MkdirAll(filepath.Dir(dst), 0o700); err != nil {
		return err
	}
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	tmp := dst + ".rush.tmp"
	out, err := os.OpenFile(tmp, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o600)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	if err := out.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, dst)
}

var (
	_ agent.Dispatcher = Adapter{}
	_ agent.Stopper    = Adapter{}
	_ agent.Remover    = Adapter{}
	_ agent.Replier    = Adapter{}
	_ agent.Attacher   = Adapter{}
	_ agent.Loginer    = Adapter{}
	_ agent.Screener   = Adapter{}
	_ agent.Mover      = Adapter{}
)
