package state

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A config.json that can't be read comes back as it was last read whole,
// rather than empty, so the next save doesn't wipe your settings.
func TestLoadFallsBackToLastGood(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	s := Load()
	s.Config.SortBy = "cost"
	s.Config.Folders = append(s.Config.Folders, s.Config.ActiveAccount())
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	Load() // read whole: kept as the last good copy
	path := filepath.Join(dir, "config.json")
	if err := os.WriteFile(path, []byte(`{"sortBy":"cost"}  "groupBy":`), 0o600); err != nil {
		t.Fatal(err)
	}
	s = Load()
	if s.Config.SortBy != "cost" || len(s.Config.Folders) != 1 {
		t.Fatalf("got %+v, want the last good config", s.Config)
	}
	if _, err := os.Stat(path + ".broken"); err != nil {
		t.Fatalf("the unreadable config wasn't kept aside: %v", err)
	}
	if m, _ := filepath.Glob(filepath.Join(dir, "*.tmp")); len(m) > 0 {
		t.Fatalf("temp files left behind: %v", m)
	}
}

// A config from before accounts were grouped by agent loads with what it
// meant, and saves in a shape an older rush still reads: its folders
// under "accounts", and staying put as stayOnAccount.
func TestMigrateAccounts(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	path := filepath.Join(dir, "config.json")
	old := `{"accounts":[{"name":"work","configDir":"/x/.claude"},{"name":"old","configDir":"/x/.claude-old"}],"stayOnAccount":true,"groupBy":"account"}`
	if err := os.WriteFile(path, []byte(old), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	c := s.Config
	if c.SwitchOnLimit != OnLimitOff || c.GroupBy != "agent" || len(c.Folders) != 2 {
		t.Fatalf("migrated to %+v", c)
	}
	KeepBefore("before-accounts")
	s.Config.SetSwitchOnLimit(OnLimitAgent)
	s.Config.NoteSignIn(SignIn{Kind: "codex", ID: "acct-1", Email: "me@example.com"}, "")
	if got := s.Config.NoteSignIn(SignIn{Kind: "codex", ID: "acct-2", Email: "me@example.org"}, ""); got.Name != "me-2" {
		t.Fatalf("a second account with the same name is called %q", got.Name)
	}
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	KeepBefore("before-accounts") // never overwritten
	b, _ := os.ReadFile(path + ".before-accounts")
	if string(b) != old {
		t.Fatalf("the config before was kept as %s", b)
	}
	var raw map[string]any
	b, _ = os.ReadFile(path)
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		t.Fatal(err)
	}
	if _, ok := raw["accounts"]; !ok || raw["stayOnAccount"] != nil {
		t.Fatalf("saved %s", b)
	}
	s.Config.ForgetSignIn("codex", "acct-1")
	if len(s.Config.SignInsOf("codex")) != 1 {
		t.Fatalf("forgot the wrong one: %+v", s.Config.SignIns)
	}
}

func TestConfigOnDiskSeesOnlyOthersChanges(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	s := Load()
	s.Config.DockLines = 4
	if err := writeJSON(filepath.Join(Dir(), "config.json"), s.Config); err != nil {
		t.Fatal(err)
	}
	if _, ok := ConfigOnDisk(); ok {
		t.Fatal("rush's own save showed as a change")
	}
	if err := os.WriteFile(filepath.Join(Dir(), "config.json"), []byte(`{"dockLines": 9}`), 0o600); err != nil {
		t.Fatal(err)
	}
	b, ok := ConfigOnDisk()
	if !ok {
		t.Fatal("an outside edit didn't show")
	}
	if err := s.Reload(b); err != nil || s.Config.DockLines != 9 {
		t.Fatalf("reload: %v, dock %d", err, s.Config.DockLines)
	}
	if _, ok := ConfigOnDisk(); ok {
		t.Fatal("a reloaded config showed again")
	}
}
