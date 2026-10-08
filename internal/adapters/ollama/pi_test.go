package ollama

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	piad "github.com/0xdeafcafe/rush/internal/adapters/pi"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

func TestPiModelsPointAtOllama(t *testing.T) {
	dir := t.TempDir()
	models := []Model{
		{Name: "qwen3.8:27b-mlx", Capabilities: []string{"tools", "thinking", "vision"}, MaxContext: 262144, Context: 65536},
		{Name: "qwen2.5:1.5b", Capabilities: []string{"tools"}},
	}
	if err := writePiModels(dir, models, "http://127.0.0.1:11434"); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(dir, "models.json"))
	if err != nil {
		t.Fatal(err)
	}
	var got struct {
		Providers map[string]struct {
			BaseURL string          `json:"baseUrl"`
			API     string          `json:"api"`
			APIKey  string          `json:"apiKey"`
			Compat  map[string]bool `json:"compat"`
			Models  []piModel       `json:"models"`
		} `json:"providers"`
	}
	if err := jsonx.Unmarshal(b, &got); err != nil {
		t.Fatal(err)
	}
	p, ok := got.Providers["ollama"]
	if !ok || p.BaseURL != "http://127.0.0.1:11434/v1" || p.API != "openai-completions" || p.APIKey == "" {
		t.Fatalf("provider: %+v", got)
	}
	if p.Compat["supportsDeveloperRole"] || p.Compat["supportsReasoningEffort"] {
		t.Errorf("compat: %v", p.Compat)
	}
	if len(p.Models) != 2 {
		t.Fatalf("models: %+v", p.Models)
	}
	first := p.Models[0]
	if first.ID != "qwen3.8:27b-mlx" || !first.Reasoning || first.ContextWindow != 65536 || first.MaxTokens != 16384 ||
		!slices.Contains(first.Input, "image") {
		t.Errorf("the loaded model: %+v", first)
	}
	if small := p.Models[1]; small.Reasoning || small.ContextWindow != 0 || slices.Contains(small.Input, "image") {
		t.Errorf("a model with no window known: %+v", small)
	}
}

func TestPiIsOllamaInPi(t *testing.T) {
	if agent.HarnessOf(PiKind) != piad.Kind || agent.ProviderOf(PiKind) != string(Kind) {
		t.Errorf("harness %s, provider %s", agent.HarnessOf(PiKind), agent.ProviderOf(PiKind))
	}
	if !slices.Contains(agent.Harnesses(string(Kind)), PiKind) {
		t.Error("not among Ollama's harnesses")
	}
	if agent.ProgramOf(PiKind) != "" {
		t.Error("a shell running ollama would be taken for Ollama in Pi")
	}
}

// TestPiLive runs a turn of Pi on the real Ollama, then reads the session
// back: RUSH_OLLAMA_PI_LIVE=<model>.
func TestPiLive(t *testing.T) {
	model := os.Getenv("RUSH_OLLAMA_PI_LIVE")
	if model == "" {
		t.Skip("set RUSH_OLLAMA_PI_LIVE to a model to run Pi against Ollama")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()
	home := t.TempDir()
	p := agent.Profile{Kind: PiKind, Dir: home}
	conn, err := PiAdapter{}.Start(ctx, agent.StartOptions{Dir: t.TempDir(), Model: model, Profile: p})
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if err := conn.Send(agent.Input{Text: "say hi"}); err != nil {
		t.Fatal(err)
	}
	var said strings.Builder
	for ev := range conn.Events() {
		t.Logf("%T %+v", ev, ev)
		switch e := ev.(type) {
		case event.Init:
			if e.Model != model {
				t.Errorf("session runs %q, not %q", e.Model, model)
			}
		case event.Message:
			for _, part := range e.Parts {
				if e.Role == "assistant" && part.Kind == event.Text {
					said.WriteString(part.Text)
				}
			}
		case event.TurnEnd:
			if e.Reason != "done" || strings.TrimSpace(said.String()) == "" {
				t.Fatalf("turn ended %+v, having said %q", e, said.String())
			}
			t.Logf("said %q", said.String())
			past := PiAdapter{}.Past(p)
			if len(past) != 1 || past[0].Kind != PiKind || past[0].Model != model {
				t.Fatalf("past: %+v", past)
			}
			if h, err := (PiAdapter{}).History(past[0], time.Time{}); err != nil || len(h) < 3 {
				t.Fatalf("history: %d events, %v", len(h), err)
			}
			return
		}
	}
	t.Fatal("the session ended before its turn did")
}
