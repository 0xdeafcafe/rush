// Package codex is OpenAI's Codex CLI as a rush adapter. It drives
// `codex app-server`, Codex's own JSON-RPC protocol, rather than ACP,
// because only it reports the account's rate-limit windows.
package codex

import (
	"context"
	"os"
	"path/filepath"
	"strconv"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/failure"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Kind is Codex's.
const Kind agent.Kind = "codex"

func init() {
	agent.Register(Adapter{})
	// Codex's own words for a sign-in it can no longer refresh.
	failure.Add(failure.Auth, "refresh_token_reused", "refresh token", "sign in again")
}

// Adapter is the Codex CLI.
type Adapter struct{}

func (Adapter) Kind() agent.Kind { return Kind }

// LoginHint is how to sign Codex in again from a terminal.
func (Adapter) LoginHint() string { return "run `codex login` in a terminal" }

// Warm: OpenAI keeps its models warm in the cloud.
func (Adapter) Warm() {}

// ProjectFolder is the folder it keeps in a project.
func (Adapter) ProjectFolder() string { return ".codex" }
func (Adapter) Name() string          { return "Codex" }

// Maker is OpenAI, whose models Codex runs.
func (Adapter) Maker() string { return "OpenAI" }

// KeyEnv is where Codex reads an OpenAI API key from.
func (Adapter) KeyEnv() string { return "OPENAI_API_KEY" }

// Speaks is the one API Codex talks to.
func (Adapter) Speaks() string { return "OpenAI's Responses API" }

// Program is codex.
func (Adapter) Program() (string, []string) { return "codex", []string{".codex/bin"} }

// Published is @openai/codex.
func (Adapter) Published() agent.Published { return agent.Published{NPM: "@openai/codex"} }

// features are what Rush currently integrates from app-server, not the full
// native Codex capability set. Conversation rollback is distinct from file rewind.
var features = map[agent.Feature]agent.Support{
	agent.FeatureRun: agent.Yes, agent.FeaturePrompt: agent.Yes, agent.FeaturePort: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureFork: agent.Yes,
	agent.FeatureInterrupt: agent.Yes, agent.FeatureGuide: agent.Yes.With("steered into the turn"), agent.FeatureModel: agent.Yes.With("from the next turn"),
	agent.FeatureEffort: agent.Yes, agent.FeatureModes: agent.Yes.With("read-only, auto, full-access"),
	agent.FeatureImages: agent.Yes, agent.FeatureQuestions: agent.Yes, agent.FeatureSubagents: agent.Yes.With("its spawn_agent threads"), agent.FeatureContext: agent.Yes.With("how full it is, not what fills it"),
	agent.FeatureCompact: agent.Yes, agent.FeatureMCP: agent.Yes, agent.FeatureHandoffIn: agent.Yes, agent.FeatureBackground: agent.Yes.With("its background terminals and spawned threads"),
	agent.FeatureLive: agent.Yes, agent.FeatureHistory: agent.Yes,
	agent.FeatureSwitch: agent.Yes, agent.FeatureSignIn: agent.Yes, agent.FeatureQuota: agent.Yes,
	agent.FeatureCommands: agent.Planned, agent.FeaturePricing: agent.Planned,
	agent.FeatureMemory: agent.Planned.With("its AGENTS.md files"), agent.FeatureSettings: agent.Planned.With("config.toml"),
}

func (Adapter) Features() map[agent.Feature]agent.Support { return features }
func (Adapter) Level() agent.Level                        { return agent.LevelTested }

// Profiles is CODEX_HOME, or ~/.codex, when it exists and codex is
// installed: a folder left behind by an uninstalled codex lists nothing.
func (Adapter) Profiles() []agent.Profile {
	if !agent.Installed(Kind) {
		return nil
	}
	dir := os.Getenv("CODEX_HOME")
	if dir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil
		}
		dir = filepath.Join(home, ".codex")
	}
	if fi, err := os.Stat(dir); err != nil || !fi.IsDir() {
		return nil
	}
	return []agent.Profile{{Kind: Kind, Name: filepath.Base(dir), Dir: dir}}
}

// Quota reads the limits of the account p is signed in to.
func (Adapter) Quota(ctx context.Context, p agent.Profile, _ agent.Account) (usage.Quota, error) {
	return Quota(ctx, p, "")
}

var _ agent.QuotaSource = Adapter{}

// UseReset spends one of the signed-in account's earned limit resets.
func (Adapter) UseReset(ctx context.Context, p agent.Profile, id string) (string, error) {
	return UseReset(ctx, p, "", id)
}

var _ agent.ResetSpender = Adapter{}

// Start runs a thread in its own app-server.
func (Adapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.APIKey != "" {
		o.Env, o.Flags = append(o.Env, "OPENAI_API_KEY="+o.APIKey), append(KeyFlags(), o.Flags...)
	}
	if o.Subagents > 0 {
		o.Flags = append([]string{"-c", "agents.max_threads=" + strconv.Itoa(o.Subagents)}, o.Flags...)
	}
	c, err := Start(ctx, o)
	if err != nil {
		return nil, err
	}
	return c, nil
}

// SpawnCommand is a run of Codex a shell can hand rush to host.
func (Adapter) SpawnCommand() (cmd, modelFlag string) { return `codex exec "<task>"`, "-m" }

// keyProvider is the model provider rush gives Codex to pay with an API
// key: OpenAI's own, read from OPENAI_API_KEY rather than the ChatGPT
// sign-in, so the same CODEX_HOME keeps its threads either way.
const keyProvider = "rush-openai"

// KeyFlags are app-server's config overrides that pay per token with the
// key in OPENAI_API_KEY.
func KeyFlags() []string {
	return []string{
		"-c", "model_provider=" + strconv.Quote(keyProvider),
		"-c", "model_providers." + keyProvider + `={name="OpenAI",base_url="https://api.openai.com/v1",env_key="OPENAI_API_KEY",wire_api="responses"}`,
	}
}
