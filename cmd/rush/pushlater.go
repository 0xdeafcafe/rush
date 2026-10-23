package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/pushlater"
	"github.com/0xdeafcafe/rush/internal/state"
)

// pushLater is `rush push-later [git push args]`: the push the gate's hook
// held, sent once the branch is quiet. `--now` sends this branch's held push
// at once; `--run <key>` is the waiter, started by the first.
func pushLater(args []string) int {
	if len(args) == 2 && args[0] == "--run" {
		return pushWait(args[1])
	}
	now := len(args) > 0 && args[0] == "--now"
	if now {
		args = args[1:]
	}
	dir, branch, err := gitWhere()
	if err != nil {
		return gitPush(".", args)
	}
	key := pushlater.Key(dir, branch)
	if now {
		h, err := pushlater.Take(key)
		if err != nil {
			fmt.Fprintln(os.Stderr, "rush push-later:", pushlater.ErrNone)
			return 1
		}
		return gitPush(h.Dir, h.Args)
	}
	if pushlater.Off() || pushlater.Protected(branch) || branch == defaultBranch(dir) {
		return gitPush(dir, args)
	}
	h := pushlater.Hold{Dir: dir, Branch: branch, Args: args, Session: os.Getenv("RUSH_SESSION"), At: time.Now()}
	if _, err := pushlater.Write(h); err != nil {
		return gitPush(dir, args) // can't hold it: send it
	}
	if err := startWaiter(key); err != nil {
		_, _ = pushlater.Take(key)
		return gitPush(dir, args)
	}
	fmt.Printf("push held: rush sends %s once it has had no new push for %s; you'll get a message if it fails (rush push-later --now sends it now)\n",
		branch, pushlater.Quiet)
	return 0
}

// gitWhere is the repository's top folder and the branch checked out.
func gitWhere() (dir, branch string, err error) {
	out, err := exec.Command("git", "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD").Output()
	if err != nil {
		return "", "", err
	}
	f := strings.Fields(string(out))
	if len(f) != 2 || f[1] == "HEAD" {
		return "", "", errors.New("not on a branch")
	}
	return f[0], f[1], nil
}

// defaultBranch is the remote's default branch, "" when unknown.
func defaultBranch(dir string) string {
	out, err := exec.Command("git", "-C", dir, "symbolic-ref", "--short", "refs/remotes/origin/HEAD").Output()
	if err != nil {
		return ""
	}
	return strings.TrimPrefix(strings.TrimSpace(string(out)), "origin/")
}

// gitPush runs git push args in dir, its output passed through.
func gitPush(dir string, args []string) int {
	cmd := exec.Command("git", append([]string{"-C", dir, "push"}, args...)...)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) {
			return ee.ExitCode()
		}
		fmt.Fprintln(os.Stderr, "rush push-later:", err)
		return 1
	}
	return 0
}

// startWaiter starts `rush push-later --run key`, in its own session so it
// outlives the agent's shell.
func startWaiter(key string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	log, err := os.OpenFile(filepath.Join(state.Dir(), "push-later", key+".log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer log.Close()
	cmd := exec.Command(exe, "push-later", "--run", key)
	cmd.Stdout, cmd.Stderr = log, log
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}

// pushWait is the waiter: one per held branch.
func pushWait(key string) int {
	if !pushlater.Lock(key, host.Alive) {
		return 0
	}
	defer pushlater.Unlock(key)
	pushlater.Waiter{
		Now:   time.Now,
		Sleep: time.Sleep,
		Push: func(h pushlater.Hold) error {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
			defer cancel()
			out, err := exec.CommandContext(ctx, "git", append([]string{"-C", h.Dir, "push"}, h.Args...)...).CombinedOutput()
			f := map[string]any{"branch": h.Branch, "args": strings.Join(h.Args, " ")}
			if err != nil {
				f["err"] = err.Error()
				err = fmt.Errorf("%w\n%s", err, strings.TrimSpace(string(out)))
			}
			if h.Session != "" {
				host.LogEvent(h.Session, "push-sent", f)
			}
			return err
		},
		Fail: func(h pushlater.Hold, err error) {
			if h.Session == "" {
				return
			}
			msg := fmt.Sprintf("rush held your `git push` of %s and sent it once the branch was quiet, but it failed:\n\n%s\n\nFix it, then push again.", h.Branch, err)
			cmd := exec.Command(os.Args[0], "session", "send", h.Session)
			cmd.Stdin = strings.NewReader(msg)
			_ = cmd.Run()
		},
	}.Run(key)
	return 0
}
