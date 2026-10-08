// Package cross runs one provider's models in another's harness, where the
// provider serves the API that harness speaks: DeepSeek and GLM in Claude
// Code, on their Anthropic-compatible APIs, and Anthropic, OpenAI,
// DeepSeek and GLM in Pi. Each is paid for per token with the provider's
// API key, which Settings › Providers keeps.
//
// Pairs that would need a translator aren't here: Claude Code speaks only
// Anthropic's API, which OpenAI doesn't serve, and Codex only OpenAI's
// Responses API, which no other provider but Ollama does. Ollama's own
// pairs are in its package, fitted to a model on this machine.
package cross

import (
	"context"
	"fmt"
	"maps"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/photon/jsonx"
	claudead "github.com/0xdeafcafe/rush/internal/adapters/claude"
	piad "github.com/0xdeafcafe/rush/internal/adapters/pi"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Pair is a provider's models in another's harness.
type Pair struct {
	kind     agent.Kind
	name     string
	provider string     // whose models, as its agent's kind
	harness  agent.Kind // the program that runs them: claude or pi
	url      string     // the provider's API, in the harness's words
	// Pi: the provider it knows by name (anthropic, openai), or one of
	// rush's own in models.json, speaking api, with models to offer.
	piProvider, piAPI string
	models            []string
	// plan runs the provider on the plan the harness is signed in to (Pi's
	// own /login), in the harness's own folder, with no key at all.
	plan bool
}

// pairs are every provider in a harness it speaks to.
// ponytail: DeepSeek's and GLM's model ids are as they were named in 2026;
// read the provider's /models when they go stale.
var pairs = []Pair{
	{kind: "deepseek-claude", name: "DeepSeek in Claude Code", provider: "deepseek", harness: claudead.Kind, url: "https://api.deepseek.com/anthropic"},
	{kind: "glm-claude", name: "GLM in Claude Code", provider: "glm", harness: claudead.Kind, url: "https://api.z.ai/api/anthropic"},
	{kind: "anthropic-pi", name: "Anthropic in Pi", provider: string(claudead.Kind), harness: piad.Kind, piProvider: "anthropic"},
	{kind: "openai-pi", name: "OpenAI in Pi", provider: "codex", harness: piad.Kind, piProvider: "openai"},
	{kind: "anthropic-plan-pi", name: "Anthropic plan in Pi", provider: string(claudead.Kind), harness: piad.Kind, piProvider: "anthropic", plan: true},
	{kind: "openai-plan-pi", name: "OpenAI plan in Pi", provider: "codex", harness: piad.Kind, piProvider: "openai-codex", plan: true},
	{kind: "deepseek-pi", name: "DeepSeek in Pi", provider: "deepseek", harness: piad.Kind, url: "https://api.deepseek.com/v1",
		piProvider: "rush-deepseek", piAPI: "openai-completions", models: []string{"deepseek-chat", "deepseek-reasoner"}},
	{kind: "glm-pi", name: "GLM in Pi", provider: "glm", harness: piad.Kind, url: "https://api.z.ai/api/coding/paas/v4",
		piProvider: "rush-glm", piAPI: "openai-completions", models: []string{"glm-4.6", "glm-4.5-air"}},
}

func init() {
	for i := range pairs {
		agent.Register(pairs[i])
	}
}

func (p Pair) Kind() agent.Kind  { return p.kind }
func (p Pair) Name() string      { return p.name }
func (p Pair) Rides() agent.Kind { return p.harness }
func (p Pair) Provider() string  { return p.provider }
func (p Pair) OnPlan() bool      { return p.plan }

// Program is the harness's: it runs wherever that's installed.
func (p Pair) Program() (string, []string) {
	a, _ := agent.Get(p.harness)
	return a.(agent.Programmer).Program()
}

// Features are the harness's, less the account it would be signed in to:
// the key pays, and has no window to show.
func (p Pair) Features() map[agent.Feature]agent.Support {
	h, _ := agent.Get(p.harness)
	out := maps.Clone(h.Features())
	// Its sessions are in a folder of its own, found by nothing yet, and
	// the harness's commands and prices are for its own provider.
	for _, f := range []agent.Feature{agent.FeatureLive, agent.FeatureHistory, agent.FeatureCommands} {
		out[f] = agent.No.With("not for a provider in another's harness yet")
	}
	out[agent.FeaturePricing] = agent.No.With("the provider's own prices, which rush doesn't know")
	out[agent.FeatureQuota] = agent.No.With("paid per token with the API key")
	out[agent.FeatureSwitch] = agent.No.With("one API key, kept in Settings › Providers")
	out[agent.FeatureSignIn] = agent.No.With("an API key, not a sign-in")
	out[agent.FeatureEffort] = agent.No.With("as the provider serves it")
	if p.plan {
		out[agent.FeatureQuota] = agent.No.With("your plan's, which Pi doesn't report")
		out[agent.FeatureSwitch] = agent.No.With("the one Pi's /login signed in")
		out[agent.FeatureSignIn] = agent.No.With("Pi's own /login, in Pi")
	}
	if p.harness == claudead.Kind {
		out[agent.FeatureRewind] = agent.No.With("provider-specific transcript branching and file rewind are not integrated")
	}
	return out
}

func (Pair) Level() agent.Level { return agent.LevelPreview }

// Profiles is rush's own folder for the pair, when its harness is here.
func (p Pair) Profiles() []agent.Profile {
	if !agent.Installed(p.kind) {
		return nil
	}
	return []agent.Profile{{Kind: p.kind, Name: p.name, Dir: p.home()}}
}

// home is the harness's folder for the pair: apart from your own, so it
// reads none of your plugins, and its sessions never mix with them. On a
// plan it's Pi's own, where its /login keeps the sign-in.
func (p Pair) home() string {
	if p.plan {
		return piad.Home()
	}
	return filepath.Join(state.Dir(), string(p.kind))
}

// Start runs the harness on the provider's API, with its key.
func (p Pair) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) { //nolint:gocritic // agent.Driver's signature
	if p.plan {
		return p.startOnPlan(ctx, o)
	}
	key := firstNonEmpty(o.APIKey, state.APIKey(p.provider))
	if key == "" {
		return nil, fmt.Errorf("%s has no API key: add one in Settings › Providers", agent.ProviderLabel(p.provider))
	}
	if o.Profile.Dir == "" {
		o.Profile.Dir = p.home()
	}
	o.APIKey, o.Effort = "", ""
	o.Binary = agent.Path(p.harness)
	if p.harness == claudead.Kind {
		o.Env = append(append([]string(nil), o.Env...), claudeEnv(p.url, key, o.Model)...)
		return claudead.Adapter{}.Start(ctx, o)
	}
	env, err := p.piSetup(o.Profile.Dir, key)
	if err != nil {
		return nil, err
	}
	o.Env = append(append([]string(nil), o.Env...), env...)
	if o.Model == "" && len(p.models) > 0 {
		o.Model = p.models[0]
	}
	o.Flags = append([]string{"--provider", p.piProvider}, o.Flags...)
	return piad.Start(ctx, &o)
}

// startOnPlan runs Pi on the provider's plan its /login signed in to,
// with the provider's key variable emptied so a key never pays instead.
func (p Pair) startOnPlan(ctx context.Context, o agent.StartOptions) (agent.Conn, error) { //nolint:gocritic // as Start
	if o.Profile.Dir == "" {
		o.Profile.Dir = p.home()
	}
	o.APIKey, o.Effort = "", ""
	o.Binary = agent.Path(p.harness)
	o.Env = append(append([]string(nil), o.Env...), agent.KeyEnv(p.provider)+"=")
	o.Flags = append([]string{"--provider", p.piProvider}, o.Flags...)
	return piad.Start(ctx, &o)
}

// claudeAliases are the models Claude Code names for itself; each goes to
// the one picked, so none asks the provider for a Claude it hasn't.
var claudeAliases = []string{
	"ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL",
	"ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL",
	"ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL",
}

// claudeEnv points Claude Code at url with key, never your own Anthropic
// key or login. With no model picked, the provider maps Claude's names to
// its own.
func claudeEnv(url, key, model string) []string {
	env := []string{"ANTHROPIC_BASE_URL=" + url, "ANTHROPIC_AUTH_TOKEN=" + key, "ANTHROPIC_API_KEY=",
		"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1"}
	if model != "" {
		for _, a := range claudeAliases {
			env = append(env, a+"="+model)
		}
	}
	return env
}

// piSetup readies Pi's folder for the pair: the key in the variable Pi
// reads for a provider it knows, else a models.json naming rush's own
// provider, with the key in it (only you can read it).
func (p Pair) piSetup(dir, key string) ([]string, error) {
	if p.piAPI == "" {
		return []string{agent.KeyEnv(p.provider) + "=" + key}, nil
	}
	models := make([]map[string]any, 0, len(p.models))
	for _, m := range p.models {
		models = append(models, map[string]any{"id": m, "name": m, "input": []string{"text"}})
	}
	cfg := map[string]any{"providers": map[string]any{p.piProvider: map[string]any{
		"baseUrl": p.url, "api": p.piAPI, "apiKey": key, "models": models,
	}}}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	b, err := jsonx.MarshalIndent(cfg)
	if err != nil {
		return nil, err
	}
	tmp := filepath.Join(dir, "models.json.tmp")
	if err := os.WriteFile(tmp, append(b, '\n'), 0o600); err != nil {
		return nil, err
	}
	return nil, os.Rename(tmp, filepath.Join(dir, "models.json"))
}

func firstNonEmpty(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

var (
	_ agent.Adapter    = Pair{}
	_ agent.Driver     = Pair{}
	_ agent.Programmer = Pair{}
	_ agent.Rider      = Pair{}
	_ agent.Provided   = Pair{}
)
