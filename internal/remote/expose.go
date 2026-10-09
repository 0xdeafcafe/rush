package remote

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// TailscalePort is the https port serve asks for on the tailnet unless
// remote.json says another. Not 443, which may be serving something of
// yours; and serves on one machine with the same port don't share it:
// the second finds it held and says so.
const TailscalePort = 8443

// Expose runs, while the tailscale and cloudflared plugins are on, what
// puts serve (listening on port) in reach of other machines: the
// installed tailscale's `serve` for this port, and the installed
// cloudflared with the tunnel's token file. It looks at the plugins every
// few seconds, so turning one on or off takes effect without a restart.
// Each thing is serve's own and is the only thing taken down: never
// another serve mapping, tunnel, or agent. The channel closes once ctx is
// done and they are down.
func Expose(ctx context.Context, cfg Config, port int, log io.Writer) <-chan struct{} {
	done := make(chan struct{})
	ts := &exposure{name: "tailscale", log: log, up: func(ctx context.Context) error {
		return tailscaleUp(ctx, cfg.TailscalePortOr(), "http://127.0.0.1:"+strconv.Itoa(port), log)
	}}
	cf := &exposure{name: "cloudflared", log: log, up: func(ctx context.Context) error { return cloudflaredRun(ctx, cfg, port, log) }}
	go func() {
		defer close(done)
		tick := time.NewTicker(3 * time.Second)
		defer tick.Stop()
		for {
			ts.set(ctx, plugin.BundledOn(PluginTailscale) && ctx.Err() == nil)
			cf.set(ctx, plugin.BundledOn(PluginCloudflared) && ctx.Err() == nil)
			if ctx.Err() != nil {
				ts.wait()
				cf.wait()
				return
			}
			select {
			case <-ctx.Done():
			case <-tick.C:
			}
		}
	}()
	return done
}

// exposure is one way of being reached, started and stopped as its plugin is.
type exposure struct {
	name string
	log  io.Writer
	up   func(ctx context.Context) error // runs until ctx ends, or fails; undoes what it did itself
	stop context.CancelFunc
	wg   sync.WaitGroup
}

func (e *exposure) set(ctx context.Context, on bool) {
	if on == (e.stop != nil) {
		return
	}
	if !on {
		e.stop()
		e.stop = nil
		e.wg.Wait()
		return
	}
	e.wg.Wait() // a worker turned off is gone, its mapping with it, before the next starts
	var c context.Context
	c, e.stop = context.WithCancel(ctx)
	e.wg.Add(1)
	go func() {
		defer e.wg.Done()
		for c.Err() == nil {
			if err := e.up(c); err != nil && c.Err() == nil {
				fmt.Fprintf(e.log, "rush serve: %s: %v\n", e.name, err)
				select { // a failure is tried again, not spun on
				case <-c.Done():
				case <-time.After(15 * time.Second):
				}
				continue
			}
			break
		}
	}()
}

func (e *exposure) wait() {
	if e.stop != nil {
		e.stop()
		e.stop = nil
	}
	e.wg.Wait()
}

func tailscaleBin() (string, error) {
	if p, err := exec.LookPath("tailscale"); err == nil {
		return p, nil
	}
	const app = "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
	if _, err := os.Stat(app); err == nil {
		return app, nil
	}
	return "", fmt.Errorf("tailscale isn't installed")
}

// unmapTailscale takes down a tailnet mapping to this machine's serve
// (cfg's listen port), and nothing else: for when serve has been stopped
// without the chance to.
func unmapTailscale(cfg Config) {
	_, port, err := net.SplitHostPort(cfg.Listen)
	bin, berr := tailscaleBin()
	if err != nil || berr != nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	maps := map[int]string{cfg.TailscalePortOr(): "http://127.0.0.1:" + port}
	if _, sport, err := net.SplitHostPort(cfg.SSHListenOr()); err == nil {
		maps[cfg.SSHPortOr()] = "tcp://127.0.0.1:" + sport
	}
	for ts, target := range maps {
		if _, ours, err := mapping(ctx, bin, ts, target); err == nil && ours {
			_ = tailscale(ctx, bin, "serve", portFlag(ts, target), "off").Run()
		}
	}
}

// tailscale is a tailscale command. The Mac app's binary is its CLI only
// from a terminal or with TAILSCALE_BE_CLI set; started any other way (by
// launchd, say) it tries to start the app and prints that instead.
func tailscale(ctx context.Context, bin string, args ...string) *exec.Cmd {
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Env = append(os.Environ(), "TAILSCALE_BE_CLI=1")
	return cmd
}

// serveStatus is the part of `tailscale serve status --json` that says
// what's on an https port.
type serveStatus struct {
	TCP map[string]struct{ TCPForward string }
	Web map[string]struct {
		Handlers map[string]struct{ Proxy string }
	}
}

// mapping reads what the tailnet's port holds: whether anything does, and
// whether it is exactly one "/" proxying to target, or for a tcp:// target
// a forward to it.
func mapping(ctx context.Context, bin string, port int, target string) (held, ours bool, err error) {
	out, err := tailscale(ctx, bin, "serve", "status", "--json").Output()
	if err != nil {
		return false, false, fmt.Errorf("tailscale serve status: %w", err)
	}
	var st serveStatus
	if err := json.Unmarshal(out, &st); err != nil {
		return false, false, fmt.Errorf("tailscale serve status: %w", err)
	}
	p := strconv.Itoa(port)
	tcp, held := st.TCP[p]
	if fwd, ok := strings.CutPrefix(target, "tcp://"); ok {
		return held, held && tcp.TCPForward == fwd, nil
	}
	for host, w := range st.Web {
		if !strings.HasSuffix(host, ":"+p) {
			continue
		}
		held = true
		if h := w.Handlers; len(h) == 1 && h["/"].Proxy == target {
			ours = true
		}
	}
	return held, ours, nil
}

// portFlag is tailscale serve's flag for the tailnet port target is on.
func portFlag(tsPort int, target string) string {
	if strings.HasPrefix(target, "tcp://") {
		return "--tcp=" + strconv.Itoa(tsPort)
	}
	return "--https=" + strconv.Itoa(tsPort)
}

// tailscaleUp maps the tailnet's port to target (serve, or its SSH), and blocks until
// ctx ends, then takes the mapping down. The port isn't ours because
// it's the default: a mapping already there is someone's, and is left
// alone. Only a mapping this call made, and that still reads as it
// made it, is removed.
func tailscaleUp(ctx context.Context, tsPort int, target string, log io.Writer) error {
	bin, err := tailscaleBin()
	if err != nil {
		return err
	}
	flag := portFlag(tsPort, target)
	// Startup runs to its end even if ctx ends meanwhile: a mapping made
	// and not looked at again would be left behind.
	cctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), time.Minute)
	defer cancel()
	held, ours, err := mapping(cctx, bin, tsPort, target)
	if err != nil {
		return err
	}
	var out []byte
	switch {
	case ours:
		// A serve here that ended without taking it down left it: it points
		// at the port this serve holds, so it's this serve's to keep.
		out = []byte("(kept the mapping a serve here left)\n")
	case held:
		return fmt.Errorf("tailnet https port %d already serves something; not touching it (pick another with remote.json's tailscalePort, or `tailscale serve %s off` if it's stale)", tsPort, flag)
	default:
		if out, err = tailscale(cctx, bin, "serve", "--bg", flag, target).CombinedOutput(); err != nil {
			return fmt.Errorf("tailscale serve: %v: %s", err, out)
		}
	}
	// Ours from here, if it reads as we set it.
	if _, ours, err := mapping(cctx, bin, tsPort, target); err != nil || !ours {
		return fmt.Errorf("tailscale serve didn't leave the mapping it should (%v); leaving it", err)
	}
	fmt.Fprintf(log, "rush serve: on the tailnet, %s\n%s", strings.TrimPrefix(flag, "--"), out)
	<-ctx.Done()
	dctx, dcancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer dcancel()
	if _, ours, err := mapping(dctx, bin, tsPort, target); err != nil || !ours {
		fmt.Fprintf(log, "rush serve: tailnet port %d no longer reads as ours; leaving it\n", tsPort)
		return nil
	}
	if out, err := tailscale(dctx, bin, "serve", flag, "off").CombinedOutput(); err != nil {
		fmt.Fprintf(log, "rush serve: tailscale serve off: %v: %s\n", err, out)
	}
	return nil
}

// cloudflaredRun runs the tunnel until ctx ends. cloudflared 2025.4.0 or
// later reads the token from a file; its hostname in Cloudflare points at
// http://127.0.0.1:<port>.
func cloudflaredRun(ctx context.Context, cfg Config, port int, log io.Writer) error {
	tok := cfg.Cloudflared
	if tok == "" {
		tok = filepath.Join(state.Dir(), "cloudflared-token")
	}
	if _, err := os.Stat(tok); err != nil {
		return fmt.Errorf("no tunnel token file at %s", tok)
	}
	bin, err := exec.LookPath("cloudflared")
	if err != nil {
		return fmt.Errorf("cloudflared isn't installed")
	}
	cmd := exec.CommandContext(ctx, bin, "tunnel", "--no-autoupdate", "run", "--token-file", tok)
	cmd.Stdout, cmd.Stderr = log, log
	cmd.WaitDelay = 5 * time.Second
	fmt.Fprintf(log, "rush serve: cloudflared tunnel (its hostname should point at http://127.0.0.1:%d)\n", port)
	if err := cmd.Run(); err != nil && ctx.Err() == nil {
		return err
	}
	return nil
}
