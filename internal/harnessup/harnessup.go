// Package harnessup finds out which of the agents' programs rush runs
// have a newer version out, and updates them.
//
// It runs each program's --version, asks Homebrew or the package's
// registry for the newest, and remembers the newest for Every, as update
// does for rush itself. Every function here waits on programs or the
// network: never call one on the UI goroutine.
package harnessup

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/netwatch"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Every is how long a newest version is kept before it's asked for again.
const Every = 4 * time.Hour

// Job is what looking for newer harnesses is called where rush shows what
// waits on the network.
const Job = "Harness update check"

// State is how a harness stands against its newest version.
type State int

const (
	Unknown State = iota // either version couldn't be found
	UpToDate
	Available
)

// Status is one harness on this machine.
type Status struct {
	Kind      agent.Kind
	Name      string
	Path      string // the real program, not rush's stand-in for it
	Installed string
	Latest    string
	Update    Update // empty Argv: no way known to update it
}

// State is whether a newer version is out.
func (s Status) State() State {
	switch {
	case s.Installed == "" || s.Latest == "":
		return Unknown
	case Newer(s.Latest, s.Installed):
		return Available
	}
	return UpToDate
}

type cache map[agent.Kind]struct {
	Latest string    `json:"latest"`
	At     time.Time `json:"at"`
}

func cachePath() string { return filepath.Join(state.Dir(), "harnessup.json") }

// Check is every installed harness's status. The newest versions are
// asked for only when none was kept in the last Every (or force).
func Check(ctx context.Context, force bool) []Status {
	var c cache
	if b, err := os.ReadFile(cachePath()); err == nil {
		_ = jsonx.Unmarshal(b, &c)
	}
	if c == nil {
		c = cache{}
	}
	var rows []Status
	seen := map[string]bool{}
	for _, a := range agent.InstalledAll() {
		if agent.CurrentKind(a.Kind()) != a.Kind() {
			continue
		}
		k := a.Kind()
		path := realProgram(k)
		if path == "" || agent.ProgramOf(k) == "" || seen[path] {
			continue
		}
		seen[path] = true
		rows = append(rows, Status{Kind: k, Name: a.Name(), Path: path})
	}
	var mu sync.Mutex
	var wg sync.WaitGroup
	online := true
	for i := range rows {
		wg.Add(1)
		go func(r *Status) {
			defer wg.Done()
			real, _ := filepath.EvalSymlinks(r.Path)
			src := sourceOf(real)
			pub := published(r.Kind)
			r.Update = planFor(pub, r.Path, src)
			r.Installed = versionOf(ctx, r.Path)
			mu.Lock()
			e, fresh := c[r.Kind], false
			fresh = e.Latest != "" && time.Since(e.At) < Every && !force
			mu.Unlock()
			if !fresh {
				if !netwatch.Run(Job) {
					mu.Lock()
					online = false
					mu.Unlock()
				} else {
					l := latestOf(ctx, pub, src)
					netwatch.Done(Job, nil)
					mu.Lock()
					if l != "" {
						e.Latest, e.At = l, time.Now()
						c[r.Kind] = e
					}
					mu.Unlock()
				}
			}
			mu.Lock()
			r.Latest = c[r.Kind].Latest
			mu.Unlock()
		}(&rows[i])
	}
	wg.Wait()
	if b, err := jsonx.Marshal(c); err == nil && online {
		_ = os.MkdirAll(state.Dir(), 0o700)
		_ = os.WriteFile(cachePath(), b, 0o600)
	}
	return rows
}

// published is where agent k's program says it's published, if it does.
func published(k agent.Kind) agent.Published {
	if p, ok := agent.As[agent.Publisher](k); ok {
		return p.Published()
	}
	return agent.Published{}
}

// realProgram is where agent k's program is, past rush's stand-ins.
func realProgram(k agent.Kind) string {
	p := agent.Path(k)
	if p == "" {
		return ""
	}
	if filepath.Clean(filepath.Dir(p)) != filepath.Clean(host.ShimDir()) {
		return p
	}
	name := agent.ProgramOf(k)
	for _, d := range filepath.SplitList(host.WithoutShims(os.Getenv("PATH"))) {
		c := filepath.Join(d, name)
		if fi, err := os.Stat(c); err == nil && !fi.IsDir() && fi.Mode()&0o111 != 0 {
			return c
		}
	}
	return ""
}

// versionOf is what program path says its version is, or empty.
func versionOf(ctx context.Context, path string) string {
	ctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, path, "--version")
	cmd.Env = append(os.Environ(), "NO_COLOR=1", "CI=1")
	out, _ := cmd.Output()
	return Parse(string(out))
}

// Run updates the harness s describes, and is how it went: an error
// carries the last of what the updater said.
func Run(ctx context.Context, s Status) error {
	if len(s.Update.Argv) == 0 {
		return errors.New("rush doesn't know how " + s.Name + " was installed: update it the way you installed it")
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()
	argv := append([]string(nil), s.Update.Argv...)
	if !filepath.IsAbs(argv[0]) {
		p, ok := agent.Find(argv[0])
		if !ok {
			return fmt.Errorf("%s isn't on your PATH", argv[0])
		}
		argv[0] = p
	}
	out, err := exec.CommandContext(ctx, argv[0], argv[1:]...).CombinedOutput()
	if err != nil {
		tail := strings.TrimSpace(string(out))
		if i := strings.LastIndex(tail, "\n"); i >= 0 {
			tail = tail[i+1:]
		}
		return fmt.Errorf("%s: %s", s.Update.Label, cmp.Or(tail, err.Error()))
	}
	return nil
}
