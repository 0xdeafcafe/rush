package vibe

import (
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/acp"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/pelletier/go-toml/v2"
)

func readConfig(p agent.Profile) map[string]any {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	path := filepath.Join(dir, "config.toml")
	if custom := os.Getenv("VIBE_CONFIG_PATH"); custom != "" {
		path = custom
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var cfg map[string]any
	if toml.Unmarshal(b, &cfg) != nil {
		return nil
	}
	return cfg
}
func (a Adapter) Choices() agent.Choices {
	return agent.Choices{Efforts: []agent.Choice{{ID: "off"}, {ID: "low"}, {ID: "medium"}, {ID: "high"}, {ID: "max"}}}
}
func (Adapter) ListModels(p agent.Profile) []agent.Choice {
	cfg := readConfig(p)
	// Vibe merges model overrides with its built-in catalog. Onboarding normally
	// writes no models section, so a TOML-only list wrongly appears empty.
	out := []agent.Choice{{ID: "mistral-medium-3.5", Note: "Mistral Medium 3.5 · Vibe built-in"}, {ID: "local", Note: "Devstral · local llama.cpp server"}}
	add := func(id string, m map[string]any) {
		name, _ := m["name"].(string)
		if alias, ok := m["alias"].(string); ok && alias != "" {
			id = alias
		}
		if id == "" {
			id = name
		}
		if id != "" {
			note, _ := m["display_name"].(string)
			if note == "" {
				note = name
			}
			choice := agent.Choice{ID: id, Note: note}
			for i, old := range out {
				if old.ID == id {
					if note == "" {
						choice.Note = old.Note
					}
					out[i] = choice
					return
				}
			}
			out = append(out, choice)
		}
	}
	switch models := cfg["models"].(type) {
	case []any:
		for _, raw := range models {
			if m, ok := raw.(map[string]any); ok {
				add("", m)
			}
		}
	case map[string]any:
		for id, raw := range models {
			if m, ok := raw.(map[string]any); ok {
				add(id, m)
			}
		}
	}
	for _, key := range []string{"routed_extra_models"} {
		if models, ok := cfg[key].([]any); ok {
			for _, raw := range models {
				if m, ok := raw.(map[string]any); ok {
					add("", m)
				}
			}
		}
	}
	if id, _ := cfg["active_model"].(string); id != "" {
		found := false
		for _, m := range out {
			found = found || m.ID == id
		}
		if !found {
			out = append(out, agent.Choice{ID: id, Note: "configured active model"})
		}
	}
	if allowed, ok := cfg["allowed_models"].([]any); ok && len(allowed) > 0 {
		var filtered []agent.Choice
		for _, model := range out {
			for _, raw := range allowed {
				pattern, ok := raw.(string)
				if !ok {
					continue
				}
				pattern = strings.TrimSpace(pattern)
				match := false
				if expr, regex := strings.CutPrefix(pattern, "re:"); regex {
					if re, err := regexp.Compile("(?i)^(?:" + expr + ")$"); err == nil {
						match = re.MatchString(model.ID)
					}
				} else {
					match, _ = filepath.Match(strings.ToLower(pattern), strings.ToLower(model.ID))
				}
				if match {
					filtered = append(filtered, model)
					break
				}
			}
		}
		// Vibe keeps all models when a stale filter matches none.
		if len(filtered) > 0 {
			out = filtered
		}
	}

	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}
func (Adapter) Live(agent.Profile) []agent.Session { return nil }
func (a Adapter) Past(p agent.Profile) []agent.Session {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	root := filepath.Join(dir, "logs", "session")
	if logging, ok := readConfig(p)["session_logging"].(map[string]any); ok {
		if path, _ := logging["save_dir"].(string); path != "" {
			if strings.HasPrefix(path, "~/") {
				h, _ := os.UserHomeDir()
				path = filepath.Join(h, path[2:])
			}
			root = path
		}
	}
	paths, _ := filepath.Glob(filepath.Join(root, "*", "meta.json"))
	var out []agent.Session
	for _, path := range paths {
		var m struct {
			ID          string            `json:"session_id"`
			Title       string            `json:"title"`
			Start       string            `json:"start_time"`
			End         string            `json:"end_time"`
			Environment map[string]string `json:"environment"`
			Config      struct {
				Model string `json:"active_model"`
			} `json:"config"`
		}
		b, err := os.ReadFile(path)
		if err != nil || jsonx.Unmarshal(b, &m) != nil || m.ID == "" {
			continue
		}
		transcript := filepath.Join(filepath.Dir(path), "messages.jsonl")
		st, err := os.Stat(transcript)
		if err != nil {
			continue
		}
		start, _ := time.Parse(time.RFC3339Nano, m.Start)
		out = append(out, agent.Session{Kind: Kind, Profile: p, ID: m.ID, Name: m.Title, Cwd: m.Environment["working_directory"], Model: m.Config.Model, Transcript: transcript, State: "done", CreatedAt: start, UpdatedAt: st.ModTime()})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].UpdatedAt.After(out[j].UpdatedAt) })
	return out
}
func (Adapter) History(s agent.Session, before time.Time) ([]event.Event, error) {
	return acp.ReadMessagesBefore(s.Transcript, before)
}
func (a Adapter) Features() map[agent.Feature]agent.Support {
	out := maps.Clone(a.Agent.Features())
	out[agent.FeatureSignIn] = agent.Yes.With("native terminal login; no account swapping")
	out[agent.FeatureEffort] = agent.Yes.With("model-supported thinking levels")
	out[agent.FeatureHistory] = agent.Yes.With("saved local conversations")
	return out
}

var _ agent.ModelLister = Adapter{}
var _ agent.Discoverer = Adapter{}
var _ agent.HistoryReader = Adapter{}

func (Adapter) HistoryTail(s agent.Session, most int64) ([]event.Event, bool, error) {
	return acp.ReadMessagesTail(s.Transcript, most)
}

var _ agent.TailReader = Adapter{}
