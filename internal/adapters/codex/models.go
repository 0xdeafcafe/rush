package codex

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// cachedModel is one model in Codex's models cache.
type cachedModel struct {
	Slug          string `json:"slug"`
	Visibility    string `json:"visibility"`
	Description   string `json:"description"`
	ContextWindow int64  `json:"context_window"`
}

// readModels are every model Codex's cache in dir has, listed or not.
func readModels(dir string) []cachedModel {
	b, err := os.ReadFile(filepath.Join(dir, "models_cache.json"))
	if err != nil {
		return nil
	}
	var cache struct {
		Models []cachedModel `json:"models"`
	}
	if jsonx.Unmarshal(b, &cache) != nil {
		return nil
	}
	return cache.Models
}

// ListModels are the models Codex offers the account signed in at p, as
// its models cache last had them: the ones its picker lists, with what
// each is for.
func (Adapter) ListModels(p agent.Profile) []agent.Choice {
	var out []agent.Choice
	for _, m := range readModels(p.Dir) {
		if m.Slug != "" && m.Visibility == "list" {
			out = append(out, agent.Choice{ID: m.Slug, Note: strings.TrimSuffix(strings.TrimSpace(m.Description), "."), Context: m.ContextWindow})
		}
	}
	return out
}

// modelID is name as Codex knows it: a model of models, or the one whose
// last word it is (astra for gpt-6-astra). With no models known, name
// stands as it is; else one Codex doesn't have is an error that says
// which it has, rather than a turn its server refuses.
func modelID(name string, models []cachedModel) (string, error) {
	if name == "" || len(models) == 0 {
		return name, nil
	}
	low := strings.ToLower(name)
	var by []string
	for _, m := range models {
		if strings.EqualFold(m.Slug, name) {
			return m.Slug, nil
		}
		if strings.HasSuffix(strings.ToLower(m.Slug), "-"+low) {
			by = append(by, m.Slug)
		}
	}
	if len(by) == 1 {
		return by[0], nil
	}
	var listed []string
	for _, m := range models {
		if m.Visibility == "list" {
			listed = append(listed, m.Slug)
		}
	}
	return "", fmt.Errorf("codex has no model %q; it has %s", name, strings.Join(listed, ", "))
}
