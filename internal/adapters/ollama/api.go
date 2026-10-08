package ollama

import (
	"bytes"
	"context"
	"fmt"
	"net/http"
	"os"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/uithread"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// server is where Ollama listens: OLLAMA_HOST as Ollama itself reads it,
// else its own default.
func server() string {
	h := strings.TrimSpace(os.Getenv("OLLAMA_HOST"))
	if h == "" {
		return "http://127.0.0.1:11434"
	}
	if !strings.Contains(h, "://") {
		h = "http://" + h
	}
	if strings.HasPrefix(h, "http://0.0.0.0") {
		h = "http://127.0.0.1" + strings.TrimPrefix(h, "http://0.0.0.0")
	}
	if strings.Count(h, ":") < 2 { // no port
		h += ":11434"
	}
	return strings.TrimRight(h, "/")
}

// Model is what Ollama says of one of its models: /api/show, and /api/ps
// when it's loaded.
type Model struct {
	Name         string
	Capabilities []string
	Family       string
	Params       string // "31.1B"
	Quant        string // "Q4_K_M"
	MaxContext   int    // what the model was trained to
	Context      int    // what the server loaded it with; 0 when not loaded
	VRAM         int64
}

// Can is whether the model has capability c: tools, thinking, vision.
func (m Model) Can(c string) bool {
	for _, x := range m.Capabilities {
		if x == c {
			return true
		}
	}
	return false
}

// CanCode excludes decision classifiers whose base model may still
// advertise tools, although the fine-tune only returns choice letters.
func (m Model) CanCode() bool { return m.Can("tools") && !m.Can("decision") }

// client has no timeout of its own: loading a model takes as long as it
// takes, so each caller bounds its own.
var client = &http.Client{}

func call(ctx context.Context, method, path string, body, out any) error {
	var r *bytes.Reader
	if body != nil {
		b, err := jsonx.Marshal(body)
		if err != nil {
			return err
		}
		r = bytes.NewReader(b)
	} else {
		r = bytes.NewReader(nil)
	}
	req, err := http.NewRequestWithContext(ctx, method, server()+path, r)
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := client.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return fmt.Errorf("ollama: %s took too long: %w", path, ctx.Err())
		}
		return fmt.Errorf("Ollama isn't running at %s: start it with `ollama serve`, or open the Ollama app", server())
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		var e struct{ Error string }
		_ = jsonx.Decode(resp.Body, &e)
		if e.Error == "" {
			e.Error = resp.Status
		}
		return fmt.Errorf("ollama: %s", e.Error)
	}
	if out == nil {
		return nil
	}
	return jsonx.Decode(resp.Body, out)
}

// names are the models the server has.
func names(ctx context.Context) ([]string, error) {
	var tags struct {
		Models []struct{ Name string }
	}
	if err := call(ctx, http.MethodGet, "/api/tags", nil, &tags); err != nil {
		return nil, err
	}
	out := make([]string, 0, len(tags.Models))
	for _, m := range tags.Models {
		out = append(out, m.Name)
	}
	return out, nil
}

// loaded are the models in memory now, with the context each was loaded
// with.
func loaded(ctx context.Context) (map[string]Model, error) {
	var ps struct {
		Models []struct {
			Name          string
			SizeVRAM      int64 `json:"size_vram"`
			ContextLength int   `json:"context_length"`
		}
	}
	if err := call(ctx, http.MethodGet, "/api/ps", nil, &ps); err != nil {
		return nil, err
	}
	out := map[string]Model{}
	for _, m := range ps.Models {
		out[m.Name] = Model{Name: m.Name, Context: m.ContextLength, VRAM: m.SizeVRAM}
	}
	return out, nil
}

// show is what the server knows of model name.
func show(ctx context.Context, name string) (Model, error) {
	var s struct {
		Capabilities []string
		Details      struct {
			Family            string
			ParameterSize     string `json:"parameter_size"`
			QuantizationLevel string `json:"quantization_level"`
		}
		ModelInfo map[string]any `json:"model_info"`
	}
	if err := call(ctx, http.MethodPost, "/api/show", map[string]string{"model": name}, &s); err != nil {
		return Model{}, err
	}
	m := Model{Name: name, Capabilities: s.Capabilities, Family: s.Details.Family,
		Params: s.Details.ParameterSize, Quant: s.Details.QuantizationLevel}
	shown.Store(name, m.Can("vision"))
	for k, v := range s.ModelInfo {
		if strings.HasSuffix(k, ".context_length") {
			if f, ok := v.(float64); ok {
				m.MaxContext = int(f)
			}
		}
	}
	return m, nil
}

// load puts model name in memory, so the first turn doesn't wait on it
// and the context it's loaded with can be read back.
func load(ctx context.Context, name string) error {
	var response struct{ Done bool }
	return call(ctx, http.MethodPost, "/api/generate", map[string]any{"model": name, "stream": false}, &response)
}

// shown is whether each model /api/show has told of reads images.
var shown, asking sync.Map

// reads is what model reads, as /api/show says: images when it has
// vision, and nothing else. A model not asked of yet is asked of in the
// background, and not known until the answer comes, so nothing waits on
// the network.
// ponytail: the first send to a model not yet shown in this process goes
// unchecked; ask when a session's model is first seen if that matters.
func reads(model string) (agent.Media, bool) {
	model = strings.TrimPrefix(model, piProvider+"/")
	if model == "" {
		return 0, false
	}
	if v, ok := shown.Load(model); ok {
		if v.(bool) {
			return agent.MediaImage, true
		}
		return 0, true
	}
	if _, busy := asking.LoadOrStore(model, true); !busy {
		go func() {
			defer asking.Delete(model)
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			_, _ = show(ctx, model)
		}()
	}
	return 0, false
}

func (Adapter) Reads(model string) (agent.Media, bool)      { return reads(model) }
func (CodexAdapter) Reads(model string) (agent.Media, bool) { return reads(model) }
func (PiAdapter) Reads(model string) (agent.Media, bool)    { return reads(model) }
func (VibeAdapter) Reads(model string) (agent.Media, bool)  { return reads(model) }

// listed caches listModels for a minute: sessions starting ask it too.
var listed struct {
	sync.Mutex
	at  time.Time
	out []agent.Choice
}

// listModels are the models this machine's Ollama has that can call
// tools, the likeliest first, each with its size and whether it's loaded.
// It asks the server, so never on the UI.
func listModels() []agent.Choice {
	uithread.Forbid("ollama.listModels")
	listed.Lock()
	defer listed.Unlock()
	if time.Since(listed.at) < time.Minute {
		return listed.out
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	models, err := InstalledModels(ctx)
	if err != nil {
		return nil
	}
	models = slices.DeleteFunc(models, func(m Model) bool { return !m.CanCode() })
	sort.SliceStable(models, func(i, j int) bool { return rank(models[i]) > rank(models[j]) })
	out := make([]agent.Choice, 0, len(models))
	for _, m := range models {
		note := strings.Join(slices.DeleteFunc([]string{m.Family, m.Params, m.Quant}, func(s string) bool { return s == "" }), " ")
		if m.Can("vision") {
			note += ", reads images"
		}
		if m.VRAM > 0 {
			note += ", loaded"
		}
		out = append(out, agent.Choice{ID: m.Name, Note: note, Context: int64(window(m))})
	}
	listed.at, listed.out = time.Now(), out
	return out
}

func (Adapter) ListModels(agent.Profile) []agent.Choice      { return listModels() }
func (CodexAdapter) ListModels(agent.Profile) []agent.Choice { return listModels() }
func (PiAdapter) ListModels(agent.Profile) []agent.Choice    { return listModels() }
func (VibeAdapter) ListModels(agent.Profile) []agent.Choice  { return listModels() }
