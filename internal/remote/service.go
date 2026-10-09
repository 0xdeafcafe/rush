package remote

import (
	"context"
	"errors"
	"fmt"
	"html"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Remote mode is rush serve kept running by the system, from login on and
// after a crash, with the tailscale plugin on: this machine's sessions are
// in reach of your devices on the tailnet whether or not rush is open.
// On a Mac that's a LaunchAgent of serve's own; elsewhere it's yours to run.

const serviceLabel = "com.github.0xdeafcafe.rush.serve"

func servicePlist() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "Library", "LaunchAgents", serviceLabel+".plist")
}

// ServiceLog is where the kept serve writes.
func ServiceLog() string { return filepath.Join(state.Dir(), "serve.log") }

// RemoteOn is whether remote mode is on: serve is kept running.
func RemoteOn() bool {
	_, err := os.Stat(servicePlist())
	return err == nil
}

// SetRemote turns remote mode on (serve kept running by exe, the tailscale
// and remote-client-web plugins on) or off (serve stopped, which takes its tailnet mapping down).
func SetRemote(on bool, exe string) error {
	if runtime.GOOS != "darwin" {
		return errors.New("remote mode keeps serve running on macOS; here, run rush serve as a service of your own")
	}
	uid := fmt.Sprint(os.Getuid())
	plist := servicePlist()
	_ = exec.Command("launchctl", "bootout", "gui/"+uid+"/"+serviceLabel).Run() // not loaded is fine
	if !on {
		if cfg, err := LoadConfig(); err == nil {
			unmapTailscale(cfg) // launchd may stop serve before it takes it down itself
		}
		if err := os.Remove(plist); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		return nil
	}
	for _, p := range []string{PluginTailscale, PluginWeb} {
		if err := plugin.SetBundled(p, true); err != nil {
			return err
		}
	}
	esc := html.EscapeString
	body := `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>` + serviceLabel + `</string>
  <key>ProgramArguments</key><array><string>` + esc(exe) + `</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>PATH</key><string>` + esc(os.Getenv("PATH")) + `</string>
    <key>RUSH_HOME</key><string>` + esc(state.Dir()) + `</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>` + esc(ServiceLog()) + `</string>
  <key>StandardErrorPath</key><string>` + esc(ServiceLog()) + `</string>
</dict></plist>
`
	if err := os.MkdirAll(filepath.Dir(plist), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(plist, []byte(body), 0o644); err != nil {
		return err
	}
	if out, err := exec.Command("launchctl", "bootstrap", "gui/"+uid, plist).CombinedOutput(); err != nil {
		return fmt.Errorf("launchctl: %v: %s", err, strings.TrimSpace(string(out)))
	}
	return nil
}

// LocalPairing asks this machine's serve for a code to pair a phone with:
// an error says serve isn't up yet, or can't be asked.
func LocalPairing(ctx context.Context) (Pairing, error) {
	var p Pairing
	err := localCall(ctx, http.MethodPost, "/api/pair", &p)
	return p, err
}

// Up is whether this machine's serve answers.
func Up(ctx context.Context) bool {
	cfg, err := LoadConfig()
	if err != nil {
		return false
	}
	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, "http://"+cfg.Listen+"/api/machines", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return false
	}
	resp.Body.Close()
	return resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusOK
}
