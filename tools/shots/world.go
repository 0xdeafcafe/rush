package main

import (
	"bufio"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The machine's own name and home, noted before the world replaces them,
// so a frame that shows either is caught.
var realHome, realUser = os.Getenv("HOME"), os.Getenv("USER")

// world is the made-up machine: a home with agents' files in it, running
// sessions' hosts, and a process table.
type world struct {
	root, home string
	now        time.Time
	sleeps     []*exec.Cmd
	lns        []net.Listener

	mu    sync.Mutex
	procs []*proc.Proc
	args  map[int][]string
	cpu   map[int]float64 // percent of a core each process uses
	start time.Time
	next  int // the next made-up pid

	// cue is when a shot's timed processes start counting (see
	// procSpec.cued), and cued those processes by pid.
	cue  time.Time
	cued map[int]procSpec

	// cfg and ov are rush's settings and what's set on agents, as saved.
	cfg state.Config
	ov  state.Overlay
}

// build makes the world at root, and points this process at it.
func build(root string) (*world, error) {
	if err := os.RemoveAll(root); err != nil {
		return nil, err
	}
	w := &world{root: root, home: filepath.Join(root, "home"), now: time.Now(), args: map[int][]string{},
		cpu: map[int]float64{}, start: time.Now(), next: 41000, cued: map[int]procSpec{}}
	for _, d := range []string{w.home, filepath.Join(w.home, ".local", "bin"), filepath.Join(root, "tmp")} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return nil, err
		}
	}
	w.env()
	if err := w.bins(); err != nil {
		return nil, err
	}
	proc.Demo = &proc.DemoTable{Procs: w.table, Args: w.args}
	if err := story(w); err != nil {
		w.close()
		return nil, err
	}
	// Sam starts rush in the checkout they work on most.
	_ = os.Chdir(filepath.Join(w.home, "src", "acme", "checkout"))
	return w, nil
}

// env is only the world's: none of the machine's agents, accounts or
// tokens reach the view.
func (w *world) env() {
	tz := os.Getenv("TZ")
	os.Clearenv()
	for k, v := range map[string]string{
		"HOME": w.home, "USER": "sam", "LOGNAME": "sam", "SHELL": "/bin/zsh",
		"PATH": filepath.Join(w.home, ".local", "bin") + ":/usr/bin:/bin:/usr/sbin:/sbin",
		"TERM": "xterm-256color", "LANG": "en_GB.UTF-8", "TMPDIR": filepath.Join(w.root, "tmp"),
		"RUSH_HOME": filepath.Join(w.home, ".config", "rush"),
		// Ollama's own server is somewhere nothing listens.
		"OLLAMA_HOST":         "127.0.0.1:9",
		"GIT_CONFIG_NOSYSTEM": "1", "GIT_AUTHOR_NAME": "Sam Rivera", "GIT_AUTHOR_EMAIL": "sam@acme.example",
		"GIT_COMMITTER_NAME": "Sam Rivera", "GIT_COMMITTER_EMAIL": "sam@acme.example",
		"TZ": tz,
	} {
		if v != "" {
			os.Setenv(k, v)
		}
	}
	_ = os.Chdir(w.home)
}

// bins are the agents' programs, as stand-ins that do nothing: installed
// as far as rush can tell.
func (w *world) bins() error {
	for _, n := range []string{"claude", "codex", "copilot", "gemini", "kimi", "opencode", "vibe-acp", "dsh", "zcode-acp-server", "ollama", "gh", "rtk"} {
		script := "#!/bin/sh\nexit 0\n"
		if n == "gh" {
			// Copilot's GitHub accounts, as gh lists them.
			script = "#!/bin/sh\nif [ \"$1 $2\" = \"auth status\" ]; then\n  cat <<'EOF'\n" + ghHosts + "\nEOF\nfi\nexit 0\n"
		}
		if err := os.WriteFile(filepath.Join(w.home, ".local", "bin", n), []byte(script), 0o755); err != nil {
			return err
		}
	}
	return nil
}

const ghHosts = `{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"srivera-acme","tokenSource":"keyring","scopes":"gist, read:org, repo, workflow","gitProtocol":"ssh"},{"state":"success","active":false,"host":"github.com","login":"samrivera","tokenSource":"keyring","scopes":"gist, read:org, repo","gitProtocol":"ssh"}]}}`

// saveJSON writes v as JSON at path, making its folder.
func saveJSON(path string, v any) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	b, err := jsonx.Marshal(v)
	if err != nil {
		return err
	}
	return os.WriteFile(path, b, 0o644)
}

// git runs git in dir, at a time of the world's.
func (w *world) git(dir string, at time.Time, args ...string) error {
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	d := at.Format(time.RFC3339)
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_DATE="+d, "GIT_COMMITTER_DATE="+d)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("git %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return nil
}

// write puts a file in a repository.
func write(dir, name, body string) error {
	p := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	return os.WriteFile(p, []byte(body), 0o644)
}

// pid is a made-up process: its parent, program, arguments, memory in MB
// and CPU in percent of a core.
func (w *world) pid(ppid int, comm string, args []string, mb, cpu float64) int {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.next += 7
	p := &proc.Proc{PID: w.next, PPID: ppid, Comm: comm, Footprint: uint64(mb * (1 << 20)), Start: w.start.Add(-time.Hour)}
	w.procs = append(w.procs, p)
	w.args[p.PID] = args
	w.cpu[p.PID] = cpu
	return p.PID
}

// live is a real process for a running session's host to be, as rush
// asks the kernel whether it's alive; the table says what it looks like.
func (w *world) live(mb, cpu float64) (int, error) {
	cmd := exec.Command("sleep", "3600")
	if err := cmd.Start(); err != nil {
		return 0, err
	}
	w.sleeps = append(w.sleeps, cmd)
	w.mu.Lock()
	defer w.mu.Unlock()
	pid := cmd.Process.Pid
	w.procs = append(w.procs, &proc.Proc{PID: pid, PPID: 1, Comm: "rush", Footprint: uint64(mb * (1 << 20)), Start: w.start.Add(-time.Hour)})
	w.args[pid] = []string{"rush", "host", "run"}
	w.cpu[pid] = cpu
	return pid, nil
}

// table is the process table as it is now: CPU time grows at each
// process's rate, so rush's sampling finds it using that much.
func (w *world) table() []*proc.Proc {
	w.mu.Lock()
	defer w.mu.Unlock()
	up := time.Since(w.start)
	out := make([]*proc.Proc, 0, len(w.procs))
	for _, p := range w.procs {
		c := *p
		c.CPUTime = time.Duration(float64(up) * w.cpu[p.PID] / 100)
		if cp, ok := w.cued[p.PID]; ok {
			at := time.Since(w.cue)
			if w.cue.IsZero() || at < cp.from || cp.to > 0 && at >= cp.to {
				continue
			}
			c.Start = w.cue.Add(cp.start)
		}
		out = append(out, &c)
	}
	return out
}

// session is a running (or stopped) rush-mode session: its info where
// rush lists them, and a host that replays it to whoever connects.
func (w *world) session(info host.Info, replay []string) error {
	d := filepath.Dir(host.SockPath(info.ID))
	if err := os.MkdirAll(d, 0o700); err != nil {
		return err
	}
	info.Proto = host.Proto
	info.Homes = true
	if err := saveJSON(filepath.Join(d, "info.json"), info); err != nil {
		return err
	}
	if info.State == "stopped" {
		return nil
	}
	b, _ := jsonx.Marshal(map[string]any{"type": "agtop_info", "info": info})
	replay = append(replay, string(b))
	ln, err := net.Listen("unix", host.SockPath(info.ID))
	if err != nil {
		return err
	}
	w.lns = append(w.lns, ln)
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer c.Close()
				for _, l := range replay {
					if _, err := c.Write([]byte(l + "\n")); err != nil {
						return
					}
				}
				// Whatever the view asks, it's kept waiting: nothing here acts.
				sc := bufio.NewScanner(c)
				for sc.Scan() {
				}
			}()
		}
	}()
	return nil
}

// save writes rush's own settings and what's set on agents.
func (w *world) save() error {
	dir := filepath.Join(w.home, ".config", "rush")
	if err := saveJSON(filepath.Join(dir, "config.json"), w.cfg); err != nil {
		return err
	}
	return saveJSON(filepath.Join(dir, "state.json"), w.ov)
}

// startCue starts the timed processes' clock, for the shot about to be
// taken.
func (w *world) startCue() {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.cue = time.Now()
}

// view is the layout a shot is taken in: "split", "list" or "agent".
func (w *world) view(v string) {
	w.cfg.SetView(v)
	_ = w.save()
}

func (w *world) close() {
	for _, ln := range w.lns {
		ln.Close()
	}
	for _, c := range w.sleeps {
		_ = c.Process.Kill()
		_ = c.Wait()
	}
}
