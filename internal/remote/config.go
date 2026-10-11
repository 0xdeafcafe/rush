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
	"sync"
	"time"

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
	// TailscalePort is the tailnet https port serve is mapped to over Tailscale
	// this serve; 8443 when zero. Serves on one machine need one each.
	TailscalePort int `json:"tailscalePort,omitempty"`
	// SSHListen is where serve's SSH listens while remote-client-ssh is on;
	// 127.0.0.1:2222 when empty. SSHPort is its port on the tailnet, 2222
	// when zero.
	SSHListen string `json:"sshListen,omitempty"`
	SSHPort   int    `json:"sshPort,omitempty"`
	// Reach is how other devices get to serve: "" this machine only,
	// "tailscale" over your tailnet, or "cloudflare" through your tunnel.
	Reach string `json:"reach,omitempty"`
	// Web lets in the web app; SSH, rush itself over SSH (sshd.go).
	Web bool `json:"web,omitempty"`
	SSH bool `json:"ssh,omitempty"`
	// AttachPeers has serve keep the peers' sessions in this machine's rush,
	// as rush remote attach does.
	AttachPeers bool `json:"attachPeers,omitempty"`
	// Cloudflared is the token file of the tunnel serve runs when
	// it's reached through Cloudflare.
	Cloudflared string `json:"cloudflaredTokenFile,omitempty"`
}

// Peer is another machine's serve.
type Peer struct {
	Name  string `json:"name"`
	URL   string `json:"url"`
	Token string `json:"token"`
}

// How serve is reached (Config.Reach).
const (
	ReachLocal      = ""
	ReachTailscale  = "tailscale"
	ReachCloudflare = "cloudflare"
)

// DefaultListen is where serve listens unless told.
const DefaultListen = "127.0.0.1:7878"

func configPath() string { return filepath.Join(state.Dir(), "remote.json") }

// LoadConfig reads remote.json; a missing one is the defaults.
func LoadConfig() (Config, error) {
	c, err := loadRaw()
	if c.Listen == "" {
		c.Listen = DefaultListen
	}
	if c.Name == "" {
		c.Name, _ = os.Hostname()
		c.Name, _, _ = strings.Cut(c.Name, ".")
	}
	return c, err
}

// loadRaw is remote.json as written, without the defaults.
func loadRaw() (Config, error) {
	var c Config
	b, err := os.ReadFile(configPath())
	if errors.Is(err, fs.ErrNotExist) {
		err = nil
	} else if err == nil {
		err = jsonx.Unmarshal(b, &c)
	}
	return c, err
}

// UpdateConfig changes remote.json with f; a running serve sees it within
// a few seconds.
func UpdateConfig(f func(*Config)) error {
	c, err := loadRaw()
	if err != nil {
		return err
	}
	f(&c)
	return SaveConfig(c)
}

// live is remote.json as it is now, read again only when it has changed:
// what serve is reached by and lets in can change while it runs.
var live struct {
	sync.Mutex
	mod  time.Time
	size int64
	c    Config
}

func current() Config {
	st, err := os.Stat(configPath())
	live.Lock()
	defer live.Unlock()
	if err != nil {
		live.mod, live.size, live.c = time.Time{}, 0, Config{}
		return live.c
	}
	if !st.ModTime().Equal(live.mod) || st.Size() != live.size {
		c, err := loadRaw()
		if err == nil {
			live.c = c
		}
		live.mod, live.size = st.ModTime(), st.Size()
	}
	return live.c
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

// SSHListenOr is where serve's SSH listens.
func (c Config) SSHListenOr() string { return or(c.SSHListen, "127.0.0.1:2222") }

// SSHPortOr is serve's SSH port on the tailnet.
func (c Config) SSHPortOr() int {
	if c.SSHPort > 0 {
		return c.SSHPort
	}
	return 2222
}
