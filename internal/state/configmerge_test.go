package state

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A setting something else wrote into config.json, which this rush hasn't
// read yet, survives this rush's next save; what this rush changed is
// saved too, and the next look at the file takes the outside one in.
func TestSaveKeepsAnOutsideEditNotYetRead(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	if err := os.WriteFile(path, []byte(`{"groupBy": "project", "dockLines": 4}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()

	// Outside: the minimap is hidden and the dock grows.
	if err := os.WriteFile(path, []byte(`{"groupBy": "project", "dockLines": 9, "hideMinimap": true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	// Here, before the reload: another grouping, and a dock of its own.
	s.Config.GroupBy = "account"
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	got, _ := os.ReadFile(path)
	for _, want := range []string{`"groupBy": "account"`, `"hideMinimap": true`, `"dockLines": 9`} {
		if !strings.Contains(string(got), want) {
			t.Fatalf("config.json is %s, want %s in it", got, want)
		}
	}

	// A second save from memory, still before the reload, keeps it too.
	s.Config.GroupBy = "status"
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	got, _ = os.ReadFile(path)
	if !strings.Contains(string(got), `"hideMinimap": true`) || !strings.Contains(string(got), `"groupBy": "status"`) {
		t.Fatalf("config.json is %s after a second save", got)
	}

	b, ok := ConfigOnDisk()
	if !ok {
		t.Fatal("the merged file didn't show as changed, so rush would never take the outside edit in")
	}
	if err := s.Reload(b); err != nil || !s.Config.HideMinimap || s.Config.DockLines != 9 || s.Config.GroupBy != "status" {
		t.Fatalf("reload: %v, %+v", err, s.Config)
	}
	if _, ok := ConfigOnDisk(); ok {
		t.Fatal("a reloaded config showed again")
	}
}

// A setting both changed is saved as this rush has it: it was set here last.
func TestSaveWinsOverAnOutsideEditOfTheSameSetting(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	if err := os.WriteFile(path, []byte(`{"groupBy": "project"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	if err := os.WriteFile(path, []byte(`{"groupBy": "folder"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s.Config.GroupBy = "account"
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	if got, _ := os.ReadFile(path); !strings.Contains(string(got), `"groupBy": "account"`) {
		t.Fatalf("config.json is %s", got)
	}
	if _, ok := ConfigOnDisk(); ok {
		t.Fatal("a save with nothing taken from outside showed as a change")
	}
}

func TestMergeOutside(t *testing.T) {
	base := []byte(`{"groupBy": "project", "hideMinimap": true, "dockLines": 4}`)
	for _, c := range []struct {
		name, disk, mine string
		want             []string
		not              []string
		ok               bool
	}{
		{"nothing outside", `{"groupBy":"project","hideMinimap":true,"dockLines":4}`, `{"groupBy": "account", "hideMinimap": true, "dockLines": 4}`, nil, nil, false},
		{"outside removed one", `{"groupBy": "project", "dockLines": 4}`, `{"groupBy": "account", "hideMinimap": true, "dockLines": 4}`,
			[]string{`"groupBy": "account"`}, []string{"hideMinimap"}, true},
		{"both changed the same", `{"groupBy": "folder", "hideMinimap": true, "dockLines": 4}`, `{"groupBy": "account", "hideMinimap": true, "dockLines": 4}`, nil, nil, false},
		{"unreadable outside", `{"groupBy": `, `{"groupBy": "account"}`, nil, nil, false},
	} {
		got, ok := mergeOutside(base, []byte(c.disk), []byte(c.mine))
		if ok != c.ok {
			t.Errorf("%s: merged %v, want %v (%s)", c.name, ok, c.ok, got)
		}
		for _, w := range c.want {
			if !strings.Contains(string(got), w) {
				t.Errorf("%s: %s lacks %s", c.name, got, w)
			}
		}
		for _, n := range c.not {
			if strings.Contains(string(got), n) {
				t.Errorf("%s: %s still has %s", c.name, got, n)
			}
		}
	}
}
