package claude

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestSettingsKeepUnknownKeys(t *testing.T) {
	dir := t.TempDir()
	orig := `{
  "model": "opus",
  "statusLine": {"type": "command", "command": "~/bin/status"},
  "permissions": {"allow": ["Bash(go test:*)"], "defaultMode": "default"},
  "env": {"FOO": "bar"},
  "enabledPlugins": {"design@x": true}
}`
	path := filepath.Join(dir, "settings.json")
	if err := os.WriteFile(path, []byte(orig), 0o600); err != nil {
		t.Fatal(err)
	}
	s, err := LoadSettings(Account{ConfigDir: dir})
	if err != nil {
		t.Fatal(err)
	}
	if s.String("model") != "opus" || s.String("permissions.defaultMode") != "default" || s.Env()["FOO"] != "bar" {
		t.Fatalf("read: %q %q %v", s.String("model"), s.String("permissions.defaultMode"), s.Env())
	}
	_ = s.Set("model", "sonnet")
	_ = s.Set("permissions.defaultMode", "plan")
	_ = s.SetEnv("CLAUDE_CODE_SUBAGENT_MODEL", "haiku")
	_ = s.SetEnv("FOO", "")
	_ = s.Set("effortLevel", "high")
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(path)
	if st.Mode().Perm() != 0o600 {
		t.Errorf("mode changed to %v", st.Mode().Perm())
	}
	var got map[string]any
	b, _ := os.ReadFile(path)
	if err := jsonx.Unmarshal(b, &got); err != nil {
		t.Fatal(err)
	}
	perms := got["permissions"].(map[string]any)
	if got["model"] != "sonnet" || got["effortLevel"] != "high" || perms["defaultMode"] != "plan" {
		t.Errorf("written: %v", got)
	}
	if allow := perms["allow"].([]any); len(allow) != 1 || allow[0] != "Bash(go test:*)" {
		t.Errorf("allow rules lost: %v", perms)
	}
	if got["statusLine"] == nil || got["enabledPlugins"] == nil {
		t.Errorf("unknown keys lost: %v", got)
	}
	env := got["env"].(map[string]any)
	if _, ok := env["FOO"]; ok || env["CLAUDE_CODE_SUBAGENT_MODEL"] != "haiku" {
		t.Errorf("env: %v", env)
	}
	// Removing the last env var removes the block.
	_ = s.SetEnv("CLAUDE_CODE_SUBAGENT_MODEL", "")
	_ = s.Save()
	b, _ = os.ReadFile(path)
	got = nil
	_ = jsonx.Unmarshal(b, &got)
	if _, ok := got["env"]; ok {
		t.Errorf("empty env should be removed: %v", got)
	}
}

func TestSettingsMissingFile(t *testing.T) {
	s, err := LoadSettings(Account{ConfigDir: t.TempDir()})
	if err != nil || s.String("model") != "" {
		t.Fatalf("%v %q", err, s.String("model"))
	}
	_ = s.Set("model", "haiku")
	if err := s.Save(); err != nil {
		t.Fatal(err)
	}
}
