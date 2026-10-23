package state

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// fakeAgent is a provider for the resolver: installed unless missing.
type fakeAgent struct {
	agent.Adapter
	kind    agent.Kind
	missing bool
}

func (f fakeAgent) Kind() agent.Kind { return f.kind }
func (f fakeAgent) Name() string     { return string(f.kind) }

type missingAgent struct{ fakeAgent }

func (missingAgent) Program() (string, []string) { return "rush-no-such-program", nil }

func init() {
	for _, k := range []agent.Kind{"claude", "pa", "pb", "pc"} {
		agent.Register(fakeAgent{kind: k})
	}
	agent.Register(missingAgent{fakeAgent{kind: "gone"}})
	agent.Recheck()
}

// A session's profile is the one picked for it, else the default;
// folders no longer pick one.
func TestProfileFor(t *testing.T) {
	c := Config{DefaultProfile: "pa"}
	for _, tc := range []struct{ cwd, explicit, want string }{
		{"/elsewhere", "", "pa"},
		{"/src", "PB", "pb"},
		{"/src", "nope", "pa"},
	} {
		if got := c.ProfileFor(tc.cwd, tc.explicit).Name; got != tc.want {
			t.Errorf("ProfileFor(%q, %q) = %q, want %q", tc.cwd, tc.explicit, got, tc.want)
		}
	}
}

// The picker starts on the first installed provider while an account of it
// has room, and moves on only when the profile mixes.
func TestPick(t *testing.T) {
	out := Room{"pa": {{ID: "a1", Current: true, Out: true, Used: 99}, {ID: "a2", Out: true, Used: 97}}}
	stay := Profile{Name: "s", Providers: []string{"gone", "pa", "pb"}}
	p, ok := stay.Pick(Room{"pa": {{ID: "a1", Current: true, Used: 50}, {ID: "a2", Used: 10}}})
	if !ok || p.Kind != "pa" || p.Account.ID != "a1" || p.Wait {
		t.Fatalf("with room, picked %+v; want the account in use", p)
	}
	p, _ = stay.Pick(Room{"pa": {{ID: "a1", Current: true, Out: true}, {ID: "a2", Used: 40}, {ID: "a3", Used: 20}}})
	if p.Kind != "pa" || p.Account.ID != "a3" {
		t.Fatalf("with the one in use out, picked %+v; want the roomiest", p)
	}
	p, _ = stay.Pick(out)
	if p.Kind != "pa" || !p.Wait || p.Account.ID != "a1" {
		t.Fatalf("staying, all out, picked %+v; want to wait on pa", p)
	}
	mix := stay
	mix.Mix = MixMix
	p, _ = mix.Pick(out)
	if p.Kind != "pb" || p.Wait {
		t.Fatalf("mixing, all out, picked %+v; want pb", p)
	}
	if _, ok := (Profile{Providers: []string{"gone"}}).Pick(nil); ok {
		t.Fatal("picked a provider that isn't installed")
	}
	if n, ok := mix.Next("pa", out); !ok || n.Kind != "pb" {
		t.Fatalf("next after pa is %+v", n)
	}
	if _, ok := mix.Next("pb", out); ok {
		t.Fatal("found a provider after the last")
	}
}

// PickFor gives an account of one provider, or says there's none.
func TestPickFor(t *testing.T) {
	p := Profile{Providers: []string{"pa", "claude", "gone"}}
	if got, ok := p.PickFor("claude", nil); !ok || got.Kind != "claude" {
		t.Fatalf("PickFor(claude) = %+v, %v", got, ok)
	}
	if _, ok := p.PickFor("pb", nil); ok {
		t.Fatal("picked a provider the profile doesn't list")
	}
	if _, ok := p.PickFor("gone", nil); ok {
		t.Fatal("picked a provider that isn't installed")
	}
	if got, ok := p.PickFor("pa", Room{"pa": {{ID: "x", Current: true, Out: true}}}); ok || !got.Wait {
		t.Fatalf("picked %+v, %v with every account out", got, ok)
	}
}

// An older config's default agent, order and choice when nearly out
// became the Default profile, and that is moved into what replaced
// profiles: its first provider the default, moving on at a limit; the
// old fields are still written.
func TestMigrateProfiles(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	path := filepath.Join(dir, "config.json")
	old := `{"dispatch":{"kind":"pa"},"switchOnLimit":"agent","agentOrder":["pb","pa"]}`
	if err := os.WriteFile(path, []byte(old), 0o600); err != nil {
		t.Fatal(err)
	}
	s := Load()
	c := &s.Config
	d := c.Default()
	if c.DefaultProfile != "pa" || c.AtLimit != AtLimitNext || !d.Builtin || d.Mix != MixMix || d.Limit() != LimitHandoff {
		t.Fatalf("migrated to %+v, default %+v", c, d)
	}
	if len(d.Providers) < 2 || d.Providers[0] != "pa" {
		t.Fatalf("providers %v", d.Providers)
	}
	// Changed here: written back where older rushes read it.
	c.AtLimit = AtLimitWait
	c.SetDefaultProvider("pb")
	if err := s.SaveConfig(); err != nil {
		t.Fatal(err)
	}
	var raw struct {
		Dispatch        Dispatch
		SwitchOnLimit   string
		StayOnAccount   bool
		AgentOrder      []string
		Profiles        []Profile
		DefaultProfile  string
		ProfilesMovedIn bool
	}
	b, _ := os.ReadFile(path)
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		t.Fatal(err)
	}
	if raw.Dispatch.Kind != "pb" || raw.SwitchOnLimit != OnLimitOff || !raw.StayOnAccount || !raw.ProfilesMovedIn || len(raw.Profiles) != 1 {
		t.Fatalf("saved %s", b)
	}
	// Loaded again, nothing is moved twice.
	s = Load()
	if s.Config.DefaultProfile != "pb" || s.Config.Default().Limit() != LimitWait {
		t.Fatalf("loaded again as %+v", s.Config)
	}
}

// The default profile you made moves in once: its provider the default,
// its harness that provider's, its model the route's, its effort the
// harness's; folders no longer pick one.
func TestMoveProfilesIn(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	old := `{"dispatch":{"kind":"claude"},"builtinProfiles":true,
		"profiles":[{"name":"work","providers":["pa","pc"],"runsIn":{"pa":"pb"},"model":"m1","effort":"high","onLimit":"wait"}],
		"defaultProfile":"work","folderRules":[{"path":"~/w","profile":"work"}]}`
	if err := os.WriteFile(filepath.Join(dir, "config.json"), []byte(old), 0o600); err != nil {
		t.Fatal(err)
	}
	c := Load().Config
	if c.DefaultProfile != "pa" || c.RunsIn["pa"] != "pb" || c.AtLimit != AtLimitWait || len(c.OldFolderRules) != 1 {
		t.Fatalf("moved in as %+v", c)
	}
	if st := c.Dispatch.StartFor("pa-pb"); st.Model != "m1" {
		t.Fatalf("route starts with %+v", st)
	}
	if hs := c.Dispatch.HarnessStart("pb"); hs.Effort != "high" {
		t.Fatalf("harness starts with %+v", hs)
	}
	if k := c.Default().Kinds(); !equal(k, []string{"pa-pb"}) {
		t.Fatalf("default runs %v", k)
	}
}

// The Default profile an older rush made, doing no more than its first
// provider does, gives way to that provider's own.
func TestMigratePlainDefault(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	old := `{"dispatch":{"kind":"claude"},"agentOrder":["claude","pa"],
		"profiles":[{"name":"Default","providers":["claude","pa"]}],"defaultProfile":"Default",
		"folderRules":[{"path":"~/w","profile":"Default"}]}`
	if err := os.WriteFile(filepath.Join(dir, "config.json"), []byte(old), 0o600); err != nil {
		t.Fatal(err)
	}
	c := Load().Config
	if len(c.OldProfiles) != 0 || c.DefaultProfile != "claude" || c.OldFolderRules[0].Profile != "claude" || !c.Default().Builtin {
		t.Fatalf("migrated to %+v", c)
	}
	// Nor from before profiles.
	if err := os.WriteFile(filepath.Join(dir, "config.json"), []byte(`{"dispatch":{"kind":"pa"}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if c := Load().Config; len(c.OldProfiles) != 0 || c.Default().Name != "pa" {
		t.Fatalf("migrated to %+v", c)
	}
}

// Every installed provider is a profile.
func TestBuiltins(t *testing.T) {
	c := Config{}
	all := c.Builtins()
	names := make([]string, 0, len(all))
	for _, p := range all {
		names = append(names, p.Name)
	}
	if !equal(names, IDs()) {
		t.Fatalf("profiles %v, ids %v", names, IDs())
	}
	if c.Default().Name != "claude" {
		t.Fatalf("default %+v", c.Default())
	}
}

func equal(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
