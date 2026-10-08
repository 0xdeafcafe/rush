package gemini

import (
	"context"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

func geminiHome() string {
	h := os.Getenv("GEMINI_CLI_HOME")
	if h == "" {
		h, _ = os.UserHomeDir()
	}
	return filepath.Join(h, ".gemini")
}
func (a Adapter) Profiles() []agent.Profile {
	if !agent.Installed(Kind) {
		return nil
	}
	return []agent.Profile{{Kind: Kind, Name: a.Title, Dir: geminiHome()}}
}
func (a Adapter) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.Profile.Dir != "" && filepath.Base(o.Profile.Dir) == ".gemini" {
		o.Env = append(append([]string(nil), o.Env...), "GEMINI_CLI_HOME="+filepath.Dir(o.Profile.Dir))
	}
	return a.Agent.Start(ctx, o)
}

// Gemini resolves these aliases using the signed-in account's model access.
func (a Adapter) Choices() agent.Choices {
	return agent.Choices{Modes: []agent.Choice{{ID: "default", Note: "ask before tools"}, {ID: "autoEdit", Note: "approve edits automatically"}, {ID: "yolo", Note: "approve all tools"}, {ID: "plan", Note: "read-only planning, when enabled"}}}
}
func (Adapter) ListModels(p agent.Profile) []agent.Choice {
	out := []agent.Choice{{ID: "auto", Note: "let Gemini select the model"}, {ID: "pro", Note: "account's Pro model"}, {ID: "flash", Note: "account's fast model"}}
	if p.Dir == "" {
		p.Dir = geminiHome()
	}
	b, err := os.ReadFile(filepath.Join(p.Dir, "settings.json"))
	if err != nil {
		return out
	}
	var settings struct {
		Model struct {
			Name string `json:"name"`
		} `json:"model"`
	}
	if jsonx.Unmarshal(b, &settings) != nil {
		return out
	}
	id := settings.Model.Name
	if id != "" && id != "auto" && id != "pro" && id != "flash" {
		out = append(out, agent.Choice{ID: id, Note: "configured model"})
	}
	return out
}
