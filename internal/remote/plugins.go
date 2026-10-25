package remote

import (
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

// The bundled plugins that are serve's switches: remote keeps serve
// running (remote mode), remote-tailscale and remote-cloudflared say how
// other devices reach it, and remote-ssh lets in rush itself over SSH.
// Each is off until you turn it on, from Settings › Remote or Settings ›
// Plugins. Only remote has anything to do as it's switched (it installs
// or removes serve's LaunchAgent); serve reads the rest every few seconds
// and does the work itself.
const (
	PluginRemote      = "remote"
	PluginTailscale   = "remote-tailscale"
	PluginCloudflared = "remote-cloudflared"
	PluginSSH         = "remote-ssh"
)

func init() {
	for _, b := range []struct {
		name, about string
		req         plugin.Requires
		switched    func(bool) error
	}{
		{PluginRemote, "Remote mode: keeps rush serve running from login on, even with rush closed, so your sessions are on your phone " +
			"and other devices. How they reach it is remote-tailscale's or remote-cloudflared's; Settings › Remote pairs a device.",
			plugin.Requires{OS: []string{"darwin"}}, switchRemote},
		{PluginTailscale, "Lets rush serve be reached over your tailnet, with the installed tailscale: https on port 8443, " +
			"and SSH on 2222 with remote-ssh. Off removes only those mappings.", plugin.Requires{}, nil},
		{PluginCloudflared, "Lets rush serve's web app be reached through your own Cloudflare tunnel, with the installed cloudflared " +
			"and the tunnel's token in cloudflared-token in rush's folder. Off stops only that tunnel.", plugin.Requires{Bin: []string{"cloudflared"}}, nil},
		{PluginSSH, "Rush itself, the whole terminal app, from any SSH app (Termius, Blink, ssh) with a paired key. Like tmux, " +
			"with no tmux: each device has its own rush, kept 30 minutes after it drops.", plugin.Requires{}, nil},
	} {
		plugin.RegisterBundle(plugin.Bundle{Optional: true, Run: runSwitch, Switched: b.switched,
			Manifest: plugin.Manifest{Name: b.name, Category: "integrations", Description: b.about,
				Command: []string{"rush"}, Requires: b.req, MemoryMB: 16}})
	}
}

// switchRemote installs serve's LaunchAgent, or removes it, as the remote
// plugin is turned on or off.
func switchRemote(on bool) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	return SetRemote(on, exe)
}

// runSwitch is a switch on its connection to the broker: it has nothing
// to do but be on.
func runSwitch(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}

// switches are the plugins' switches as last read: serve asks on every
// request, and they're read again at most every 2s.
var switches struct {
	sync.Mutex
	at time.Time
	on map[string]bool
}

// PluginOn is whether one of serve's plugins is on.
func PluginOn(name string) bool {
	switches.Lock()
	defer switches.Unlock()
	if time.Since(switches.at) > 2*time.Second {
		switches.on = map[string]bool{}
		for _, n := range []string{PluginRemote, PluginTailscale, PluginCloudflared, PluginSSH} {
			switches.on[n] = plugin.BundledOn(n)
		}
		switches.at = time.Now()
	}
	return switches.on[name]
}

// SwitchesChanged has the next PluginOn read the switches again.
func SwitchesChanged() {
	switches.Lock()
	switches.at = time.Time{}
	switches.Unlock()
}

// withSwitches is c with Reach and SSH as the plugins say: remote.json's
// own are only read once, by migrate.
func withSwitches(c Config) Config {
	c.SSH = PluginOn(PluginSSH)
	switch {
	case PluginOn(PluginTailscale):
		c.Reach = ReachTailscale
	case PluginOn(PluginCloudflared):
		c.Reach = ReachCloudflare
	default:
		c.Reach = ReachLocal
	}
	return c
}

var migrated sync.Once

// migrate moves what remote.json said before serve's switches were plugins
// into them, once: its reach and ssh, and remote mode found running.
func migrate() {
	migrated.Do(func() {
		c, err := loadRaw()
		if err != nil {
			return
		}
		if c.Reach != ReachLocal || c.SSH {
			if c.Reach == ReachTailscale {
				_ = plugin.MarkBundled(PluginTailscale, true)
			}
			if c.Reach == ReachCloudflare {
				_ = plugin.MarkBundled(PluginCloudflared, true)
			}
			if c.SSH {
				_ = plugin.MarkBundled(PluginSSH, true)
			}
			c.Reach, c.SSH = ReachLocal, false
			_ = SaveConfig(c)
		}
		if RemoteOn() && !plugin.BundledOn(PluginRemote) {
			_ = plugin.MarkBundled(PluginRemote, true)
		}
		SwitchesChanged()
	})
}
