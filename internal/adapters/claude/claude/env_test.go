package claude

import (
	"strings"
	"testing"
)

// A key in rush's own environment never reaches the sessions it starts.
func TestEnvDropsAnInheritedAPIKey(t *testing.T) {
	t.Setenv("ANTHROPIC_API_KEY", "sk-ant-api-leaked")
	t.Setenv("ANTHROPIC_AUTH_TOKEN", "leaked")
	t.Setenv("ANTHROPIC_BASE_URL", "https://example.invalid")
	t.Setenv("KEEP_ME", "1")
	for _, a := range []Account{{}, {Name: "other", ConfigDir: t.TempDir()}} {
		env := strings.Join(a.Env(), "\n")
		for _, name := range []string{"ANTHROPIC_API_KEY=", "ANTHROPIC_AUTH_TOKEN=", "ANTHROPIC_BASE_URL="} {
			if strings.Contains(env, name) {
				t.Errorf("account %q passes on %s", a.Name, name)
			}
		}
		if !strings.Contains(env, "KEEP_ME=1") {
			t.Errorf("account %q dropped an unrelated variable", a.Name)
		}
	}
}
