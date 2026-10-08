// Package update finds out whether a newer rush is out, and installs it.
//
// rush is installed with go install, so the Go module proxy is where a
// new one shows up, and go install is how it's put in place: over the
// rush that's running, wherever that lives.
package update

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime/debug"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/netwatch"
	"github.com/0xdeafcafe/rush/internal/state"
)

const (
	module = "github.com/0xdeafcafe/rush"
	pkg    = module + "/cmd/rush"
	// Every is how often rush asks the proxy; every rush shares the answer.
	Every = 6 * time.Hour
)

// Info is one version of rush, as the module proxy knows it.
type Info struct {
	Version string    `json:"Version"`
	Time    time.Time `json:"Time"`
}

// Short is v for a tag, and the commit and day for a pseudo-version.
func (i Info) Short() string {
	if m := pseudo.FindStringSubmatch(i.Version); m != nil {
		return m[2][:7] + " (" + i.Time.Local().Format("Jan 2") + ")"
	}
	return strings.TrimSuffix(i.Version, "+dirty")
}

// pseudo is a pseudo-version's time and commit.
var pseudo = regexp.MustCompile(`(\d{14})-([0-9a-f]{12})(\+dirty)?$`)

// Current is the rush that's running. Built from a checkout with go build,
// it has no version, and ok is false: there's nothing to compare.
func Current() (i Info, ok bool) {
	bi, found := debug.ReadBuildInfo()
	if !found || bi.Main.Version == "" || bi.Main.Version == "(devel)" {
		return Info{}, false
	}
	i.Version = bi.Main.Version
	if m := pseudo.FindStringSubmatch(i.Version); m != nil {
		t, err := time.Parse("20060102150405", m[1])
		return Info{Version: i.Version, Time: t}, err == nil
	}
	return i, true
}

// Latest asks the module proxy for the newest rush.
func Latest(ctx context.Context) (Info, error) {
	return fetch(ctx, "@latest")
}

func fetch(ctx context.Context, path string) (Info, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, proxy()+"/"+module+"/"+path, nil)
	if err != nil {
		return Info{}, err
	}
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return Info{}, err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return Info{}, fmt.Errorf("the Go module proxy said %s", res.Status)
	}
	var i Info
	return i, jsonx.Decode(res.Body, &i)
}

// proxy is the first proxy in GOPROXY that's a URL, or Go's own.
func proxy() string {
	for _, p := range strings.FieldsFunc(os.Getenv("GOPROXY"), func(r rune) bool { return r == ',' || r == '|' }) {
		if strings.HasPrefix(p, "https://") || strings.HasPrefix(p, "http://") {
			return strings.TrimRight(p, "/")
		}
	}
	return "https://proxy.golang.org"
}

// Newer is whether latest came out after cur. A tag's time isn't in its
// name, so it's asked for.
func Newer(ctx context.Context, cur, latest Info) bool {
	if strings.TrimSuffix(cur.Version, "+dirty") == latest.Version {
		return false
	}
	if cur.Time.IsZero() {
		if i, err := fetch(ctx, "@v/"+strings.TrimSuffix(cur.Version, "+dirty")+".info"); err == nil {
			cur.Time = i.Time
		}
	}
	return !cur.Time.IsZero() && latest.Time.After(cur.Time)
}

// cache is the last answer, shared by every rush.
type cache struct {
	Checked time.Time `json:"checked"`
	Latest  Info      `json:"latest"`
}

func cachePath() string { return filepath.Join(state.Dir(), "update.json") }

// Job is what looking for a newer rush is called where rush shows what
// waits on the network.
const Job = "Update check"

// Check is the newer rush that's out, if there is one, asking the proxy
// only when no rush has in the last Every.
func Check(ctx context.Context) (Info, bool) {
	cur, ok := Current()
	if !ok {
		return Info{}, false
	}
	var c cache
	if b, err := os.ReadFile(cachePath()); err == nil {
		_ = jsonx.Unmarshal(b, &c)
	}
	if time.Since(c.Checked) > Every || c.Latest.Version == "" {
		if !netwatch.Run(Job) {
			return Info{}, false
		}
		l, err := Latest(ctx)
		netwatch.Done(Job, err)
		if err != nil {
			return Info{}, false
		}
		c = cache{Checked: time.Now(), Latest: l}
		if b, err := jsonx.Marshal(c); err == nil {
			_ = os.MkdirAll(state.Dir(), 0o700)
			_ = os.WriteFile(cachePath(), b, 0o600)
		}
	}
	return c.Latest, Newer(ctx, cur, c.Latest)
}

// Install puts the newest rush over the one that's running, with go
// install. What's running keeps running; the next rush is the new one.
func Install(ctx context.Context) (Info, error) {
	goBin, err := exec.LookPath("go")
	if err != nil {
		return Info{}, errors.New("updating needs Go, and go isn't on your PATH — install it from https://go.dev/dl")
	}
	l, err := Latest(ctx)
	if err != nil {
		return Info{}, err
	}
	cmd := exec.CommandContext(ctx, goBin, "install", pkg+"@"+l.Version)
	cmd.Env = os.Environ()
	// Into the folder the running rush is in, so it's the one replaced;
	// one built with go build is left alone, and Go picks the folder.
	if _, ok := Current(); ok {
		if exe, err := os.Executable(); err == nil {
			if exe, err = filepath.EvalSymlinks(exe); err == nil && filepath.Base(exe) == "rush" {
				cmd.Env = append(cmd.Env, "GOBIN="+filepath.Dir(exe))
			}
		}
	}
	if out, err := cmd.CombinedOutput(); err != nil {
		return Info{}, fmt.Errorf("go install: %s", strings.TrimSpace(cmp.Or(strings.TrimSpace(string(out)), err.Error())))
	}
	c, _ := jsonx.Marshal(cache{Checked: time.Now(), Latest: l})
	_ = os.WriteFile(cachePath(), c, 0o600)
	return l, nil
}
