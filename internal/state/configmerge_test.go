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

// holdWriter keeps the write-behind writer busy until the returned func is
// called, so a test can line up what happens between a save and its write.
func holdWriter(t *testing.T) func() {
	t.Helper()
	WriteBehind()
	t.Cleanup(func() { _ = Flush(); behind.Lock(); behind.on = false; behind.Unlock() })
	release := make(chan struct{})
	queueWrite("hold", func() error { <-release; return nil })
	return func() {
		close(release)
		if err := Flush(); err != nil {
			t.Fatal(err)
		}
	}
}

// A save marshalled before a reload took in a setting written from outside,
// and written after it, keeps that setting: the reload doesn't make the
// older bytes look like they were built from the newer file.
func TestQueuedSaveKeepsAnOutsideEditReloadedBeforeItsWrite(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	if err := os.WriteFile(path, []byte(`{"groupBy": "project"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	release := holdWriter(t)

	s.Config.GroupBy = "status"
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(`{"groupBy": "project", "hideMinimap": true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	b, ok := ConfigOnDisk()
	if !ok {
		t.Fatal("the outside edit didn't show")
	}
	if err := s.Reload(b); err != nil {
		t.Fatal(err)
	}
	release()

	got, _ := os.ReadFile(path)
	for _, want := range []string{`"hideMinimap": true`, `"groupBy": "status"`} {
		if !strings.Contains(string(got), want) {
			t.Fatalf("config.json is %s, want %s in it", got, want)
		}
	}
	b, ok = ConfigOnDisk()
	if !ok {
		t.Fatal("the merged file didn't show as changed")
	}
	if err := s.Reload(b); err != nil || !s.Config.HideMinimap || s.Config.GroupBy != "status" {
		t.Fatalf("reload: %v, %+v", err, s.Config)
	}
}

// Saves that replace one another before the writer gets to them are merged
// against the file the first of them was built from: a setting changed and
// changed back here stays changed back, one changed and kept stays changed,
// and one written from outside meanwhile is kept.
func TestReplacedSavesMergeAgainstTheFirstOnesFile(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	if err := os.WriteFile(path, []byte(`{"groupBy": "project", "dockLines": 4}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	release := holdWriter(t)

	s.Config.GroupBy = "account"
	s.Config.DockLines = 7
	_ = s.SaveConfig()
	s.Config.GroupBy = "project"
	_ = s.SaveConfig()
	if err := os.WriteFile(path, []byte(`{"groupBy": "project", "dockLines": 4, "hideMinimap": true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	release()

	got, _ := os.ReadFile(path)
	for _, want := range []string{`"hideMinimap": true`, `"groupBy": "project"`, `"dockLines": 7`} {
		if !strings.Contains(string(got), want) {
			t.Fatalf("config.json is %s, want %s in it", got, want)
		}
	}
}

// A save written after an earlier one merges against what the earlier one
// wrote, so a setting changed back here is saved changed back.
func TestSaveAfterAWrittenSaveMergesAgainstIt(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	if err := os.WriteFile(path, []byte(`{"groupBy": "project"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	WriteBehind()
	t.Cleanup(func() { _ = Flush(); behind.Lock(); behind.on = false; behind.Unlock() })

	s.Config.GroupBy = "account"
	_ = s.SaveConfig()
	if err := Flush(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(strings.Replace(mustRead(t, path), "{", `{"hideMinimap": true,`, 1)), 0o600); err != nil {
		t.Fatal(err)
	}
	s.Config.GroupBy = "project"
	_ = s.SaveConfig()
	if err := Flush(); err != nil {
		t.Fatal(err)
	}
	got := mustRead(t, path)
	for _, want := range []string{`"hideMinimap": true`, `"groupBy": "project"`} {
		if !strings.Contains(got, want) {
			t.Fatalf("config.json is %s, want %s in it", got, want)
		}
	}
}

// A rush that started with no config.json keeps the settings something
// else wrote into one before this rush's first save.
func TestFirstSaveKeepsAConfigWrittenAfterStart(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	path := filepath.Join(Dir(), "config.json")
	s := Load()
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(`{"hideMinimap": true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	s.Config.GroupBy = "account"
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	got := mustRead(t, path)
	for _, want := range []string{`"hideMinimap": true`, `"groupBy": "account"`} {
		if !strings.Contains(got, want) {
			t.Fatalf("config.json is %s, want %s in it", got, want)
		}
	}
}

func mustRead(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}
