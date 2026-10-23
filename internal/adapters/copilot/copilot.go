// Package copilot is GitHub Copilot as a rush adapter: the Copilot CLI,
// run over ACP in rush mode, and Copilot's coding agent, whose sessions on
// GitHub rush lists and reads. Both are reached with your GitHub sign-in:
// gh's, when the CLI has none of its own.
package copilot

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/rush/internal/adapters/acp"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Kind is Copilot's.
const Kind agent.Kind = "copilot"

func init() { agent.Register(Adapter{}) }

// Adapter is GitHub Copilot.
type Adapter struct{}

// LoginHint is how to sign Copilot in again: it runs on gh's sign-in.
func (Adapter) LoginHint() string { return "run `gh auth login` in a terminal" }

// cli is the Copilot CLI over ACP.
var cli = acp.Agent{ID: Kind, Title: "Copilot", Command: "copilot", Args: []string{"--acp"}, Tried: agent.LevelTested,
	More: map[agent.Feature]agent.Support{
		agent.FeatureLive: agent.Yes.With("the coding agent's, on GitHub"), agent.FeatureHistory: agent.Yes,
		agent.FeatureRemote: agent.Yes.With("view only"),
		agent.FeatureSwitch: agent.Yes.With("gh's accounts"), agent.FeatureSignIn: agent.Yes,
		agent.FeatureQuota:      agent.Yes.With("premium requests"),
		agent.FeatureBackground: agent.Yes.With("async shells, ended once Copilot reads one back"),
		agent.FeaturePricing:    agent.Planned.With("its models' premium-request multipliers"),
	}}

func (Adapter) Kind() agent.Kind { return Kind }
func (Adapter) Name() string     { return "Copilot" }

// Maker is GitHub, whose service Copilot's models come through.
func (Adapter) Maker() string { return "GitHub" }

// Published is @github/copilot.
func (Adapter) Published() agent.Published { return agent.Published{NPM: "@github/copilot"} }

// Program is the Copilot CLI: what runs its sessions here. Its coding
// agent's sessions on GitHub need only gh, and are listed without it.
func (Adapter) Program() (string, []string) { return "copilot", []string{".copilot/bin"} }

// Hint is what the Copilot CLI adds to gh alone.
const Hint = "install the Copilot CLI (npm i -g @github/copilot) to run Copilot sessions here, in rush mode"

// Lesser is gh: with it alone, rush lists Copilot's coding agent's
// sessions on GitHub, its accounts and their premium requests.
func (Adapter) Lesser() (string, []string, string) { return "gh", nil, Hint }

// Features are the CLI's over ACP, and the coding agent's on GitHub. It's
// been tried lightly against the real CLI.
func (Adapter) Features() map[agent.Feature]agent.Support { return cli.Features() }
func (Adapter) Level() agent.Level                        { return cli.Level() }

// Profiles is COPILOT_HOME, or ~/.copilot, when the CLI is installed or gh
// is: the coding agent needs only a GitHub sign-in.
func (Adapter) Profiles() []agent.Profile {
	if !agent.Installed(Kind) {
		return nil
	}
	dir := os.Getenv("COPILOT_HOME")
	if dir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil
		}
		dir = filepath.Join(home, ".copilot")
	}
	return []agent.Profile{{Kind: Kind, Name: "Copilot", Dir: dir}}
}

// Start runs the Copilot CLI over ACP. Without a sign-in of its own it gets
// gh's, which it takes as GH_TOKEN.
func (Adapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.Binary == "" && !agent.Runs(Kind) {
		return nil, errors.New("copilot: " + Hint)
	}
	if !signedIn(o.Profile.Dir) && !hasToken(o.Env) {
		if tok, err := token(); err == nil {
			o.Env = append(append([]string(nil), o.Env...), "GH_TOKEN="+tok)
		}
	}
	if o.Profile.Dir != "" {
		o.Env = append(o.Env, "COPILOT_HOME="+o.Profile.Dir)
	}
	return cli.Start(ctx, o)
}

// signedIn is whether the CLI has a sign-in of its own in dir.
func signedIn(dir string) bool {
	b, err := os.ReadFile(filepath.Join(dir, "config.json"))
	return err == nil && strings.Contains(string(b), "logged_in_users")
}

func hasToken(env []string) bool {
	for _, kv := range env {
		for _, k := range []string{"COPILOT_GITHUB_TOKEN=", "GH_TOKEN=", "GITHUB_TOKEN="} {
			if strings.HasPrefix(kv, k) {
				return true
			}
		}
	}
	return false
}

var (
	_ agent.Adapter       = Adapter{}
	_ agent.Driver        = Adapter{}
	_ agent.QuotaSource   = Adapter{}
	_ agent.Discoverer    = Adapter{}
	_ agent.HistoryReader = Adapter{}
)
