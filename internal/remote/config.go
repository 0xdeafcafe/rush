// Package remote is rush serve: an HTTP server in front of this machine's
// session hosts, for the web app and for rush on other machines. A serve
// with peers is a hub, proxying to theirs. See docs/remote.md.
package remote

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Config is remote.json in rush's folder.
type Config struct {
	// Listen is where serve listens; 127.0.0.1:7878 when empty. Something
	// in front of it (a tailscale serve, a tunnel, a reverse proxy) makes it
	// reachable from elsewhere.
	Listen string `json:"listen,omitempty"`
	// Name is this machine's, as the web app lists it; the hostname when empty.
	Name string `json:"name,omitempty"`
	// Origin is the one address browsers use, e.g. https://rush.example.com:
	// a request from it is accepted, and notifications link to it.
	Origin string `json:"origin,omitempty"`
	// Workspaces, when there are any, are the only folders a remote client
	// may start a session in (and their subfolders).
	Workspaces []string `json:"workspaces,omitempty"`
	// Peers are the other machines this one shows, as a hub.
	Peers []Peer `json:"peers,omitempty"`
	// TailscalePort is the tailnet https port the tailscale plugin maps to
	// this serve; 8443 when zero. Serves on one machine need one each.
	TailscalePort int `json:"tailscalePort,omitempty"`
	// Cloudflared is the token file of the tunnel the cloudflared plugin
	// runs, when it's on.
	Cloudflared string `json:"cloudflaredTokenFile,omitempty"`
}

// Peer is another machine's serve.
type Peer struct {
	Name  string `json:"name"`
	URL   string `json:"url"`
	Token string `json:"token"`
}

// DefaultListen is where serve listens unless told.
const DefaultListen = "127.0.0.1:7878"

func configPath() string { return filepath.Join(state.Dir(), "remote.json") }

// LoadConfig reads remote.json; a missing one is the defaults.
func LoadConfig() (Config, error) {
	var c Config
	b, err := os.ReadFile(configPath())
	if errors.Is(err, fs.ErrNotExist) {
		err = nil
	} else if err == nil {
		err = jsonx.Unmarshal(b, &c)
	}
	if c.Listen == "" {
		c.Listen = DefaultListen
	}
	if c.Name == "" {
		c.Name, _ = os.Hostname()
		c.Name, _, _ = strings.Cut(c.Name, ".")
	}
	return c, err
}

// SaveConfig writes remote.json, readable only by you: it holds peers' tokens.
func SaveConfig(c Config) error {
	b, err := jsonx.MarshalIndent(c)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(state.Dir(), 0o700); err != nil {
		return err
	}
	tmp := configPath() + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, configPath())
}

// Peer is the peer called name.
func (c *Config) Peer(name string) (Peer, bool) {
	for _, p := range c.Peers {
		if p.Name == name {
			return p, true
		}
	}
	return Peer{}, false
}

// Token is this machine's serve token, made on first use. Delete the file
// and restart serve to change it.
func Token() (string, error) {
	path := filepath.Join(state.Dir(), "remote-token")
	if b, err := os.ReadFile(path); err == nil && len(strings.TrimSpace(string(b))) >= 32 {
		return strings.TrimSpace(string(b)), nil
	}
	buf := make([]byte, 32)
	_, _ = rand.Read(buf)
	t := hex.EncodeToString(buf)
	if err := os.MkdirAll(state.Dir(), 0o700); err != nil {
		return "", err
	}
	// O_EXCL: two serves starting at once agree on one token.
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		if b, rerr := os.ReadFile(path); rerr == nil {
			return strings.TrimSpace(string(b)), nil
		}
		return "", err
	}
	defer f.Close()
	_, err = f.WriteString(t + "\n")
	return t, err
}

// TailscalePortOr is the tailnet https port, 8443 unless set.
func (c Config) TailscalePortOr() int {
	if c.TailscalePort > 0 {
		return c.TailscalePort
	}
	return TailscalePort
}
