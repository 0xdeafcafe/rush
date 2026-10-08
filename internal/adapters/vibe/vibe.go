// Package vibe is Mistral Vibe as a rush adapter: vibe-acp, beside vibe,
// over ACP, in VIBE_HOME (~/.vibe). It runs on a Mistral API key Vibe
// keeps itself, which rush looks for where Vibe does. It stays out of
// sight until vibe is installed.
package vibe

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/keychain"
	"github.com/0xdeafcafe/rush/internal/adapters/acp"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Kind is Vibe's.
const Kind agent.Kind = "vibe"

func init() {
	agent.Register(Adapter{acp.Agent{
		ID: Kind, Title: "Vibe", Company: "Mistral", Command: "vibe", ACP: "vibe-acp", Home: ".vibe",
		Pub: agent.Published{PyPI: "mistral-vibe"},
		// Vibe says which plan it's on (its whoami), never how much is left;
		// a limit shows only as the error it stops a turn with.
		More: map[agent.Feature]agent.Support{agent.FeatureQuota: agent.No.With("Vibe says its plan, not what's left of it")},
		// vibe takes its model only from its config, which VIBE_* sets.
		Once: `vibe -p "<task>"`, Model: "VIBE_ACTIVE_MODEL=",
		Flags: map[string]string{"-p": "prompt", "--prompt": "prompt", "--workdir": "cwd", "--agent": "mode",
			"--auto-approve": "mode=auto-approve", "--yolo": "mode=auto-approve", "--output": "=text", "--trust": ""},
	}})
}

// Adapter is Vibe: what ACP gives it, and the key it runs on.
type Adapter struct{ acp.Agent }

// keyEnv is where Vibe reads its Mistral key first.
const keyEnv = "MISTRAL_API_KEY"

// errNoKey is a Vibe with no key rush can find.
var errNoKey = errors.New("no Mistral API key: run vibe --setup once, or set " + keyEnv)

// home is Vibe's home: VIBE_HOME, else ~/.vibe.
func home() string {
	if d := os.Getenv("VIBE_HOME"); d != "" {
		return d
	}
	h, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(h, ".vibe")
}

// Profiles is Vibe's home, when vibe is installed.
func (a Adapter) Profiles() []agent.Profile {
	if !agent.Installed(Kind) {
		return nil
	}
	return []agent.Profile{{Kind: Kind, Name: a.Title, Dir: home()}}
}

// Start runs vibe-acp in the profile's home.
func (a Adapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.Profile.Dir != "" && o.Profile.Dir != home() {
		o.Env = append(append([]string(nil), o.Env...), "VIBE_HOME="+o.Profile.Dir)
	}
	if o.Model != "" {
		o.Env = append(append([]string(nil), o.Env...), "VIBE_ACTIVE_MODEL="+o.Model)
		o.Model = "" // Vibe reads the startup model from configuration, before opening ACP.
	}
	return a.Agent.Start(ctx, o)
}

// keychained is whether Vibe keeps its key in the keychain, under its
// service or the one it used before.
var keychained = func() bool {
	return keychain.Has("ai.mistral.vibe", keyEnv) || keychain.Has("vibe", keyEnv)
}

// CheckKey finds the key where Vibe looks: the environment, the
// keychain, then its home's .env.
func (Adapter) CheckKey(p agent.Profile) error {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	if os.Getenv(keyEnv) != "" || inDotenv(filepath.Join(dir, ".env")) || keychained() {
		return nil
	}
	return errNoKey
}

// inDotenv is whether a .env file sets the key.
func inDotenv(path string) bool {
	b, _ := os.ReadFile(path)
	for _, l := range strings.Split(string(b), "\n") {
		v, ok := strings.CutPrefix(strings.TrimPrefix(strings.TrimSpace(l), "export "), keyEnv+"=")
		if ok && strings.Trim(strings.TrimSpace(v), `"'`) != "" {
			return true
		}
	}
	return false
}

var (
	_ agent.Driver     = Adapter{}
	_ agent.OnceReader = Adapter{}
	_ agent.KeyChecker = Adapter{}
)
