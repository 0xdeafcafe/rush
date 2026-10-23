package ollama

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	vibead "github.com/0xdeafcafe/rush/internal/adapters/vibe"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// VibeKind is Ollama in Vibe's.
const VibeKind agent.Kind = "ollama-vibe"

func init() { agent.Register(VibeAdapter{}) }

// VibeAdapter is Mistral Vibe on Ollama: vibe-acp, as the vibe adapter
// drives it, in a VIBE_HOME of rush's own whose config.toml points a
// provider named "ollama" at Ollama's OpenAI-compatible API, so it never
// reads the user's ~/.vibe, nor needs their Mistral key.
type VibeAdapter struct{}

func (VibeAdapter) Kind() agent.Kind { return VibeKind }
func (VibeAdapter) Name() string     { return "Ollama in Vibe" }

// Program is ollama, which it needs beside Vibe.
func (VibeAdapter) Program() (string, []string) { return Adapter{}.Program() }

// Rides Vibe: its sessions are vibe's.
func (VibeAdapter) Rides() agent.Kind { return vibead.Kind }

// Provider is Ollama, whichever harness it runs in.
func (VibeAdapter) Provider() string { return string(Kind) }

// vibeFeatures are Vibe's over ACP, less what a local model or rush's
// own folder hasn't.
var vibeFeatures = map[agent.Feature]agent.Support{
	agent.FeatureRun: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureInterrupt: agent.Yes,
	agent.FeatureModel: agent.Yes.With("any model Ollama has that calls tools"),
	agent.FeatureModes: agent.Yes.With("Vibe's own"), agent.FeatureQuestions: agent.Yes, agent.FeatureMCP: agent.Yes,
	agent.FeatureImages: agent.Yes.With("on models that take them"), agent.FeatureHandoffIn: agent.Yes,
	agent.FeaturePricing:    agent.Yes.With("free: it runs on this machine"),
	agent.FeatureEffort:     agent.No.With("a model thinks or doesn't, as it was made"),
	agent.FeatureSubagents:  agent.Yes.With("Vibe's task tool"),
	agent.FeatureBackground: agent.No.With("Vibe runs nothing beside the turn over ACP"),
	agent.FeatureQuota:      agent.No.With("no limits but this machine's"),
	agent.FeatureSwitch:     agent.No.With("no accounts, only this machine"),
	agent.FeatureSignIn:     agent.No.With("nothing to sign in to"),
	agent.FeatureHistory:    agent.Planned,
}

func (VibeAdapter) Features() map[agent.Feature]agent.Support { return vibeFeatures }
func (VibeAdapter) Level() agent.Level                        { return agent.LevelTested }

// Profiles is rush's own VIBE_HOME for Ollama, when ollama and vibe are
// both here.
func (VibeAdapter) Profiles() []agent.Profile {
	if !agent.Installed(VibeKind) {
		return nil
	}
	return []agent.Profile{{Kind: VibeKind, Name: "Ollama in Vibe", Dir: vibeHome()}}
}

// vibeHome is the VIBE_HOME sessions on Ollama keep to.
func vibeHome() string { return filepath.Join(state.Dir(), "ollama-vibe") }

// Cost is nothing: the model runs here.
func (VibeAdapter) Cost(string, usage.TokenUsage) (float64, bool) { return 0, true }

// vibeProvider is the provider rush names in Vibe's config.
const vibeProvider = "ollama"

// Start picks and loads the model, writes Vibe's config for it, then runs
// Vibe on it.
func (VibeAdapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) { //nolint:gocritic // agent.Driver's signature
	m, err := pick(ctx, o.Model)
	if err != nil {
		return nil, err
	}
	if o.Profile.Dir == "" {
		o.Profile.Dir = vibeHome()
	}
	if err := writeVibeConfig(o.Profile.Dir, toolModels(ctx, m), server()); err != nil {
		return nil, err
	}
	// The config starts it on m; setting it again would only write it back.
	o.Model, o.Effort = "", ""
	// The host hands over the adapter's own program, which is ollama's,
	// unless it was told of a vibe to run.
	if o.Binary == "" || filepath.Base(o.Binary) == "ollama" {
		o.Binary = agent.Path(vibead.Kind)
	}
	d, ok := agent.As[agent.Driver](vibead.Kind)
	if !ok {
		return nil, fmt.Errorf("rush can't run %s", vibead.Kind)
	}
	return d.Start(ctx, o)
}

// vibeConfig is Vibe's config.toml with one provider, Ollama served at
// base, and only the models given, the first active. Ollama's OpenAI API
// streams a model's thinking as "reasoning". Vibe compacts a quarter
// short of each model's window, when it's known.
func vibeConfig(models []Model, base string) string {
	q := strconv.Quote // TOML's basic strings, for the names Ollama gives
	names := make([]string, 0, len(models))
	for _, m := range models {
		names = append(names, q(m.Name))
	}
	var b strings.Builder
	fmt.Fprintf(&b, "active_model = %s\nallowed_models = [%s]\n", names[0], strings.Join(names, ", "))
	fmt.Fprintf(&b, "\n[[providers]]\nname = %s\napi_base = %s\napi_style = \"openai\"\nreasoning_field_name = \"reasoning\"\n",
		q(vibeProvider), q(base+"/v1"))
	for _, m := range models {
		fmt.Fprintf(&b, "\n[[models]]\nname = %s\nprovider = %s\nalias = %s\ninput_price = 0.0\noutput_price = 0.0\n",
			q(m.Name), q(vibeProvider), q(m.Name))
		if m.Can("vision") {
			b.WriteString("supports_images = true\n")
		}
		if n := window(m); n > 0 {
			fmt.Fprintf(&b, "auto_compact_threshold = %d\n", n-n/4)
		}
	}
	return b.String()
}

// writeVibeConfig writes config.toml into Vibe's home dir.
func writeVibeConfig(dir string, models []Model, base string) error {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	tmp := filepath.Join(dir, "config.toml.tmp")
	cfg := vibeConfig(models, base)
	// Written afresh each start, it says what General's Every agent does;
	// a top-level key, so ahead of the tables.
	switch state.Load().Config.Everywhere.Attribution {
	case "off":
		cfg = "include_commit_signature = false\n" + cfg
	case "on":
		cfg = "include_commit_signature = true\n" + cfg
	}
	if err := os.WriteFile(tmp, []byte(cfg), 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, filepath.Join(dir, "config.toml"))
}

var (
	_ agent.Adapter    = VibeAdapter{}
	_ agent.Driver     = VibeAdapter{}
	_ agent.Programmer = VibeAdapter{}
	_ agent.Pricer     = VibeAdapter{}
	_ agent.Rider      = VibeAdapter{}
	_ agent.Provided   = VibeAdapter{}
)
