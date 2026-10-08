package ollama

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	piad "github.com/0xdeafcafe/rush/internal/adapters/pi"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// PiKind is Ollama in Pi's.
const PiKind agent.Kind = "ollama-pi"

func init() { agent.Register(PiAdapter{}) }

// PiAdapter is Pi on Ollama: `pi --mode rpc`, as the pi adapter drives it,
// in a Pi config folder of rush's own whose models.json points a provider
// named "ollama" at Ollama's OpenAI-compatible API, so its sessions never
// mix with the user's ~/.pi, nor read their keys.
type PiAdapter struct{}

func (PiAdapter) Kind() agent.Kind { return PiKind }
func (PiAdapter) Name() string     { return "Ollama in Pi" }

// Program is ollama, which it needs beside Pi.
func (PiAdapter) Program() (string, []string) { return Adapter{}.Program() }

// Rides Pi: its sessions are pi's.
func (PiAdapter) Rides() agent.Kind { return piad.Kind }

// Provider is Ollama, whichever harness it runs in.
func (PiAdapter) Provider() string { return string(Kind) }

// piFeatures are Pi's, less what a local model or rush's own folder
// hasn't.
var piFeatures = map[agent.Feature]agent.Support{
	agent.FeatureRun: agent.Yes, agent.FeaturePrompt: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureFork: agent.Yes.With("the whole session, not from a message"),
	agent.FeatureInterrupt: agent.Yes, agent.FeatureModel: agent.Yes.With("any model Ollama has that calls tools"),
	agent.FeatureImages: agent.Yes.With("on models that take them"), agent.FeatureCompact: agent.Yes.With("at the window the model was loaded with"),
	agent.FeatureQuestions: agent.Yes.With("an extension's select and confirm"),
	agent.FeatureContext:   agent.Yes.With("how full it is, not what fills it"),
	agent.FeatureHandoffIn: agent.Yes, agent.FeatureLive: agent.Yes.With("sessions written to in the last two minutes"),
	agent.FeatureHistory: agent.Yes, agent.FeaturePricing: agent.Yes.With("free: it runs on this machine"),
	agent.FeatureEffort: agent.No.With("a model thinks or doesn't, as it was made"),
	agent.FeatureModes:  agent.No.With("Pi asks before nothing: every tool runs"),
	agent.FeatureQuota:  agent.No.With("no limits but this machine's"),
	agent.FeatureSwitch: agent.No.With("no accounts, only this machine"),
	agent.FeatureSignIn: agent.No.With("nothing to sign in to"),
}

func (PiAdapter) Features() map[agent.Feature]agent.Support { return piFeatures }
func (PiAdapter) Level() agent.Level                        { return agent.LevelTested }

// Profiles is rush's own Pi folder for Ollama, when ollama and pi are
// both here.
func (PiAdapter) Profiles() []agent.Profile {
	if !agent.Installed(PiKind) {
		return nil
	}
	return []agent.Profile{{Kind: PiKind, Name: "Ollama in Pi", Dir: piHome()}}
}

// piHome is the PI_CODING_AGENT_DIR sessions on Ollama keep to.
func piHome() string { return filepath.Join(state.Dir(), "ollama-pi") }

// Cost is nothing: the model runs here.
func (PiAdapter) Cost(string, usage.TokenUsage) (float64, bool) { return 0, true }

// piProvider is the provider rush names in Pi's models.json.
const piProvider = "ollama"

// Start picks and loads the model, writes Pi's models.json for it, then
// runs Pi on it.
func (PiAdapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) { //nolint:gocritic // agent.Driver's signature
	m, err := pick(ctx, strings.TrimPrefix(o.Model, piProvider+"/"))
	if err != nil {
		return nil, err
	}
	if o.Profile.Dir == "" {
		o.Profile.Dir = piHome()
	}
	if err := writePiModels(o.Profile.Dir, toolModels(ctx, m), server()); err != nil {
		return nil, err
	}
	o.Model, o.Effort = m.Name, ""
	o.Flags = append([]string{"--provider", piProvider}, o.Flags...)
	// The host hands over the adapter's own program, which is ollama's,
	// unless it was told of a pi to run.
	if o.Binary == "" || filepath.Base(o.Binary) == "ollama" {
		o.Binary = agent.Path(piad.Kind)
	}
	return piad.Start(ctx, &o)
}

// toolModels are the models a harness is told of: m, picked and loaded,
// first, then every other one Ollama has that calls tools, for a switch
// mid-session.
func toolModels(ctx context.Context, m Model) []Model {
	out := []Model{m}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	all, err := names(ctx)
	if err != nil {
		return out
	}
	for _, n := range all {
		if n == m.Name {
			continue
		}
		if o, err := show(ctx, n); err == nil && o.CanCode() {
			out = append(out, o)
		}
	}
	return out
}

// piModel is one of models.json's models.
type piModel struct {
	ID            string         `json:"id"`
	Name          string         `json:"name"`
	Reasoning     bool           `json:"reasoning"`
	Input         []string       `json:"input"`
	ContextWindow int            `json:"contextWindow,omitempty"`
	MaxTokens     int            `json:"maxTokens,omitempty"`
	Cost          map[string]int `json:"cost"`
}

// piConfig is Pi's models.json with one provider, Ollama served at base,
// and the models given. Ollama's OpenAI API takes neither the developer
// role nor reasoning_effort, so the system prompt goes as a system message
// and a model thinks as it was made to. Each model's window is the one
// it's loaded with, else the one it was trained to, so Pi compacts in
// time; it may write a quarter of it.
func piConfig(models []Model, base string) map[string]any {
	list := make([]piModel, 0, len(models))
	for _, m := range models {
		pm := piModel{ID: m.Name, Name: m.Name, Reasoning: m.Can("thinking"), Input: []string{"text"},
			Cost: map[string]int{"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}}
		if m.Can("vision") {
			pm.Input = append(pm.Input, "image")
		}
		if n := window(m); n > 0 {
			pm.ContextWindow, pm.MaxTokens = n, min(32000, n/4)
		}
		list = append(list, pm)
	}
	return map[string]any{"providers": map[string]any{piProvider: map[string]any{
		"baseUrl": base + "/v1", "api": "openai-completions", "apiKey": "ollama",
		"compat": map[string]any{"supportsDeveloperRole": false, "supportsReasoningEffort": false},
		"models": list,
	}}}
}

// writePiModels writes models.json into Pi's folder dir.
func writePiModels(dir string, models []Model, base string) error {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	b, err := jsonx.MarshalIndent(piConfig(models, base))
	if err != nil {
		return err
	}
	tmp := filepath.Join(dir, "models.json.tmp")
	if err := os.WriteFile(tmp, append(b, '\n'), 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, filepath.Join(dir, "models.json"))
}

// Live are the sessions Pi wrote to lately in rush's folder.
func (PiAdapter) Live(p agent.Profile) []agent.Session { return piOurs(piad.Adapter{}.Live(p)) }

// Past is every session in rush's folder.
func (PiAdapter) Past(p agent.Profile) []agent.Session { return piOurs(piad.Adapter{}.Past(p)) }

// History reads a session back, as Pi's own are.
func (PiAdapter) History(s agent.Session, before time.Time) ([]event.Event, error) { //nolint:gocritic // HistoryReader takes the session by value
	return piad.Adapter{}.History(s, before)
}

// RunsSession: its sessions are pi's.
func (PiAdapter) RunsSession(args []string, sessionID string) bool {
	return piad.Adapter{}.RunsSession(args, sessionID)
}

// piOurs are pi's sessions as Ollama in Pi's.
func piOurs(ss []agent.Session) []agent.Session {
	for i := range ss {
		ss[i].Kind = PiKind
		ss[i].Profile.Kind = PiKind
	}
	return ss
}

var (
	_ agent.Adapter       = PiAdapter{}
	_ agent.Driver        = PiAdapter{}
	_ agent.Programmer    = PiAdapter{}
	_ agent.Pricer        = PiAdapter{}
	_ agent.Rider         = PiAdapter{}
	_ agent.Provided      = PiAdapter{}
	_ agent.Discoverer    = PiAdapter{}
	_ agent.HistoryReader = PiAdapter{}
	_ agent.Orphans       = PiAdapter{}
)
