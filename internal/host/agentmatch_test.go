package host

import (
	"slices"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// codexPicks are spawn_agent's agents as a machine with Codex and Kimi
// lists them, named as picks names them.
func codexPicks() []pick {
	codex := runner{k: "codex", prov: "codex", models: []agent.Choice{
		{ID: "gpt-6.1-sol"}, {ID: "gpt-6-astra"}, {ID: "gpt-6-sol"}, {ID: "gpt-5.6-sol"},
	}}
	kimi := runner{k: "kimi", prov: "kimi", models: []agent.Choice{{ID: "kimi-code/k3"}, {ID: "kimi-code/kimi-for-coding"}}}
	var out []pick
	taken := map[string]bool{}
	for _, r := range []*runner{&codex, &kimi} {
		for _, c := range r.models {
			n := defName(*r, c.ID, taken)
			taken[n] = true
			out = append(out, pick{name: n, r: *r, model: c.ID})
		}
	}
	return out
}

// A session finds an agent by its listed name, its model however it's
// written, its harness, or its harness and model.
func TestFindAgent(t *testing.T) {
	ps := codexPicks()
	for _, tc := range []struct{ agent, model, kind, wantModel, wantRun string }{
		{"astra", "", "codex", "gpt-6-astra", ""},
		{"sol", "", "codex", "gpt-6.1-sol", ""},
		{"gpt-6.1-sol", "", "codex", "gpt-6.1-sol", ""},
		{"GPT-6.1-Sol", "", "codex", "gpt-6.1-sol", ""},
		{"sol6.1", "", "codex", "gpt-6.1-sol", ""},
		{"sol 6.1", "", "codex", "gpt-6.1-sol", ""},
		{"sol 6", "", "codex", "gpt-6-sol", ""},
		{"gpt 5.6 sol", "", "codex", "gpt-5.6-sol", ""},
		{"gpt-5-6-sol", "", "codex", "gpt-5.6-sol", ""},
		{"codex", "", "codex", "", ""},
		{"Codex", "", "codex", "", ""},
		{"codex astra", "", "codex", "gpt-6-astra", ""},
		{"codex:sol6.1", "", "codex", "gpt-6.1-sol", ""},
		{"codex", "astra", "codex", "gpt-6-astra", ""},
		{"codex", "gpt-6-sol", "codex", "gpt-6-sol", ""},
		{"", "sol 6.1", "codex", "gpt-6.1-sol", ""},
		{"kimi", "", "kimi", "", ""},
		{"kimi coding", "", "kimi", "kimi-code/kimi-for-coding", ""},
		{"kimi-code/k3", "", "kimi", "kimi-code/k3", ""},
		{"codex", "gpt-7-nova", "codex", "", "gpt-7-nova"}, // its harness takes or refuses it
	} {
		p, model, ok := find(ps, tc.agent, tc.model)
		if !ok {
			t.Errorf("find(%q, %q) found nothing", tc.agent, tc.model)
			continue
		}
		if string(p.r.k) != tc.kind || p.model != tc.wantModel || model != tc.wantRun {
			t.Errorf("find(%q, %q) = %s on %q, run on %q; want %s on %q, run on %q", tc.agent, tc.model, p.r.k, p.model, model, tc.kind, tc.wantModel, tc.wantRun)
		}
	}
}

// Nothing fits: the error names every agent it could have been.
func TestFindAgentNone(t *testing.T) {
	ps := codexPicks()
	for _, q := range []string{"gemini", "sol 7", "nova"} {
		if p, _, ok := find(ps, q, ""); ok {
			t.Errorf("find(%q) = %s, want nothing", q, p.name)
		}
	}
	err := noAgent("gemini", "", ps).Error()
	for _, n := range []string{"astra", "sol", "kimi-code-k3", "codex", "kimi"} {
		if !strings.Contains(err, n) {
			t.Errorf("the error leaves out %s: %s", n, err)
		}
	}
}

func TestWords(t *testing.T) {
	for in, want := range map[string][]string{
		"gpt-6.1-sol": {"gpt", "6.1", "sol"},
		"sol6.1":      {"sol", "6.1"},
		"opus[1m]":    {"opus", "1", "m"},
		"v2.":         {"v", "2"},
		"Kimi K3":     {"kimi", "k", "3"},
	} {
		if got := words(in); !slices.Equal(got, want) {
			t.Errorf("words(%q) = %q, want %q", in, got, want)
		}
	}
}
