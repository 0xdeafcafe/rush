package remote

import (
	"bytes"
	"context"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	_ "github.com/0xdeafcafe/rush/internal/bundled/remote"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

type syncBuf struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuf) Write(p []byte) (int, error) { s.mu.Lock(); defer s.mu.Unlock(); return s.b.Write(p) }
func (s *syncBuf) String() string              { s.mu.Lock(); defer s.mu.Unlock(); return s.b.String() }

// fakeTailscale is a tailscale on PATH that keeps its serve config in
// status.json and its calls in calls: just enough of `serve status
// --json`, `serve --bg --https=P URL` and `serve --https=P off`.
type fakeTailscale struct{ dir string }

func newFakeTailscale(t *testing.T) *fakeTailscale {
	t.Setenv("RUSH_HOME", t.TempDir())
	dir := t.TempDir()
	script := `#!/bin/sh
D="` + dir + `"
echo "$@" >> "$D/calls"
case "$1 $2" in
"serve status") cat "$D/status.json" 2>/dev/null || echo '{}' ;;
"serve --bg")
  [ -e "$D/failserve" ] && { echo boom >&2; exit 1; }
  p=${3#--https=}
  printf '{"TCP":{"%s":{"HTTPS":true}},"Web":{"box.ts.net:%s":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "$p" "$p" "$4" > "$D/status.json" ;;
"serve --https="*) echo '{}' > "$D/status.json" ;;
esac
`
	if err := os.WriteFile(filepath.Join(dir, "tailscale"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+":"+os.Getenv("PATH"))
	return &fakeTailscale{dir}
}

func (f *fakeTailscale) read(n string) string {
	b, _ := os.ReadFile(filepath.Join(f.dir, n))
	return string(b)
}
func (f *fakeTailscale) count(sub string) int { return strings.Count(f.read("calls"), sub) }
func (f *fakeTailscale) set(s string) {
	_ = os.WriteFile(filepath.Join(f.dir, "status.json"), []byte(s), 0o644)
}

const foreign = `{"TCP":{"8443":{"HTTPS":true}},"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}`

func until(t *testing.T, what string, ok func() bool) {
	t.Helper()
	for end := time.Now().Add(5 * time.Second); time.Now().Before(end); time.Sleep(10 * time.Millisecond) {
		if ok() {
			return
		}
	}
	t.Fatalf("timed out: %s", what)
}

func tsExposure(log *syncBuf, serve int) *exposure {
	return &exposure{name: "tailscale", log: log, up: func(ctx context.Context) error { return tailscaleUp(ctx, 8443, "http://127.0.0.1:"+strconv.Itoa(serve), log) }}
}

func TestTailscaleHappyPathRemovesOnlyItsMapping(t *testing.T) {
	f := newFakeTailscale(t)
	var log syncBuf
	e := tsExposure(&log, 7878)
	e.set(context.Background(), true)
	until(t, "mapped", func() bool { return strings.Contains(f.read("status.json"), "http://127.0.0.1:7878") })
	e.set(context.Background(), false)
	if f.read("status.json") != "{}\n" || f.count("--https=8443 off") != 1 {
		t.Fatalf("not taken down once: %q\n%s", f.read("status.json"), f.read("calls"))
	}
}

func TestTailscaleLeavesAForeignMapping(t *testing.T) {
	f := newFakeTailscale(t)
	f.set(foreign)
	var log syncBuf
	e := tsExposure(&log, 7878)
	e.set(context.Background(), true)
	until(t, "refusal logged", func() bool { return strings.Contains(log.String(), "not touching it") })
	e.set(context.Background(), false)
	if f.read("status.json") != foreign || f.count("--bg") != 0 || f.count(" off") != 0 {
		t.Fatalf("touched what wasn't ours: %q\n%s", f.read("status.json"), f.read("calls"))
	}
}

func TestTailscaleFailedStartDoesNoCleanup(t *testing.T) {
	f := newFakeTailscale(t)
	_ = os.WriteFile(filepath.Join(f.dir, "failserve"), nil, 0o644)
	var log syncBuf
	e := tsExposure(&log, 7878)
	e.set(context.Background(), true)
	until(t, "failure logged", func() bool { return strings.Contains(log.String(), "boom") })
	e.set(context.Background(), false)
	if f.count(" off") != 0 {
		t.Fatalf("cleaned up what it never made:\n%s", f.read("calls"))
	}
}

func TestTailscaleLeavesAMappingChangedUnderIt(t *testing.T) {
	f := newFakeTailscale(t)
	var log syncBuf
	e := tsExposure(&log, 7878)
	e.set(context.Background(), true)
	until(t, "mapped", func() bool { return strings.Contains(f.read("status.json"), "7878") })
	f.set(foreign) // someone replaced it
	e.set(context.Background(), false)
	if f.read("status.json") != foreign || f.count(" off") != 0 {
		t.Fatalf("removed another's mapping:\n%s", f.read("calls"))
	}
}

// A mapping to this serve's own port, left by a serve that was killed, is
// kept rather than refused, and taken down when this one stops.
func TestTailscaleKeepsWhatAKilledServeLeft(t *testing.T) {
	f := newFakeTailscale(t)
	f.set(`{"TCP":{"8443":{"HTTPS":true}},"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7878"}}}}}`)
	var log syncBuf
	e := tsExposure(&log, 7878)
	e.set(context.Background(), true)
	until(t, "kept", func() bool { return strings.Contains(log.String(), "on the tailnet") })
	e.set(context.Background(), false)
	if f.count("--bg") != 0 || f.read("status.json") != "{}\n" {
		t.Fatalf("calls:\n%s\nleft: %s", f.read("calls"), f.read("status.json"))
	}
}

// unmapTailscale removes serve's own mapping, and leaves anyone else's.
func TestUnmapTailscaleOnlyOurs(t *testing.T) {
	f := newFakeTailscale(t)
	f.set(foreign)
	unmapTailscale(Config{Listen: "127.0.0.1:7878"})
	if f.read("status.json") != foreign {
		t.Fatal("removed another's mapping")
	}
	f.set(`{"TCP":{"8443":{"HTTPS":true}},"Web":{"box.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7878"}}}}}`)
	unmapTailscale(Config{Listen: "127.0.0.1:7878"})
	if f.read("status.json") != "{}\n" {
		t.Fatalf("left: %s", f.read("status.json"))
	}
}

// Two serves on one machine that want the same tailnet port: the second
// is told it's held, and its stopping never removes the first's.
func TestTailscaleCompetingServes(t *testing.T) {
	f := newFakeTailscale(t)
	var l1, l2 syncBuf
	a, b := tsExposure(&l1, 7878), tsExposure(&l2, 7879)
	a.set(context.Background(), true)
	until(t, "a mapped", func() bool { return strings.Contains(f.read("status.json"), "7878") })
	b.set(context.Background(), true)
	until(t, "b refused", func() bool { return strings.Contains(l2.String(), "not touching it") })
	b.set(context.Background(), false)
	if !strings.Contains(f.read("status.json"), "7878") || f.count(" off") != 0 {
		t.Fatalf("b disturbed a:\n%s", f.read("calls"))
	}
	a.set(context.Background(), false)
	if f.read("status.json") != "{}\n" {
		t.Fatalf("a's mapping left: %s", f.read("status.json"))
	}
}

// Off then on at once: the old worker is gone, its mapping with it,
// before the new one maps; it can't remove the new one's.
func TestTailscaleRapidToggle(t *testing.T) {
	f := newFakeTailscale(t)
	var log syncBuf
	e := tsExposure(&log, 7878)
	ctx := context.Background()
	for i := 0; i < 5; i++ {
		e.set(ctx, true)
		until(t, "mapped", func() bool { return strings.Contains(f.read("status.json"), "7878") })
		e.set(ctx, false)
		e.set(ctx, true)
	}
	until(t, "mapped at the end", func() bool { return strings.Contains(f.read("status.json"), "7878") })
	if f.count("--bg") != f.count(" off")+1 {
		t.Fatalf("maps and removals don't pair:\n%s", f.read("calls"))
	}
	e.wait()
	if f.read("status.json") != "{}\n" {
		t.Fatalf("left behind: %s", f.read("status.json"))
	}
}

// Off by default: nothing runs. The plugin switch turns it on, and serve
// ending takes it down.
func TestExposeFollowsThePluginSwitch(t *testing.T) {
	f := newFakeTailscale(t)
	ctx, cancel := context.WithCancel(context.Background())
	var log syncBuf
	done := Expose(ctx, Config{}, 7878, &log)
	time.Sleep(300 * time.Millisecond)
	if f.read("calls") != "" {
		t.Fatalf("off, yet it ran tailscale: %q", f.read("calls"))
	}
	if err := plugin.SetBundled(PluginTailscale, true); err != nil {
		t.Fatal(err)
	}
	until(t, "mapped", func() bool { return strings.Contains(f.read("status.json"), "7878") })
	cancel()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("did not stop")
	}
	if f.read("status.json") != "{}\n" {
		t.Fatalf("mapping left: %s", f.read("status.json"))
	}
}

// A server that fails on its own, not by a signal, still ends: what it
// exposed is taken down and Run returns, rather than waiting on a ctx
// nobody cancels.
func TestRunEndsWhenServingFails(t *testing.T) {
	f := newFakeTailscale(t)
	if err := plugin.SetBundled(PluginTailscale, true); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ln.Close() // Serve fails at once
	s := &Server{Config: Config{Name: "x"}, Token: strings.Repeat("a", 64)}
	errc := make(chan error, 1)
	var log syncBuf
	go func() { errc <- s.Run(context.Background(), ln, &log) }()
	select {
	case err := <-errc:
		if err == nil {
			t.Fatal("a failed server reported success")
		}
	case <-time.After(10 * time.Second):
		t.Fatal("Run hung after the server failed")
	}
	if got := f.read("status.json"); got != "" && got != "{}\n" {
		t.Fatalf("left exposed: %s", got)
	}
}
