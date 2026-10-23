package state

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// rider is provider pr in harness h, as agent kind.
type rider struct {
	fakeAgent
	pr string
	h  agent.Kind
}

func (r rider) Provider() string  { return r.pr }
func (r rider) Rides() agent.Kind { return r.h }
func (r rider) Kind() agent.Kind  { return r.kind }
func (r rider) Name() string      { return string(r.kind) }

func init() {
	agent.Register(rider{fakeAgent{kind: "pa-pb"}, "pa", "pb"})
	agent.Recheck()
}

// A provider runs in the harness the config gives it, else its own.
func TestRunsIn(t *testing.T) {
	c := Config{}
	if p, _ := c.ProfileNamed("pa"); !equal(p.Kinds(), []string{"pa"}) {
		t.Fatalf("pa's own profile %+v", p)
	}
	c.SetRunsIn("pa", "pb")
	if p, _ := c.ProfileNamed("pa"); !p.Builtin || !equal(p.Kinds(), []string{"pa-pb"}) || !p.Has("pa-pb") || !p.Has("pa") {
		t.Fatalf("pa's own profile %+v", p)
	}
	if p, ok := c.Default().Pick(nil); !ok || p.Kind != "claude" {
		t.Fatalf("picked %+v", p)
	}
	c.SetRunsIn("pa", "pa") // its own harness: the default again
	if _, ok := c.RunsIn["pa"]; ok {
		t.Fatalf("RunsIn kept its default: %v", c.RunsIn)
	}
	// A rider's kind names its provider's profile in that harness.
	if p, ok := c.ProfileNamed("pa-pb"); !ok || !equal(p.Kinds(), []string{"pa-pb"}) {
		t.Fatalf("pa-pb's profile %+v", p)
	}
}

// splitAgent is a provider paid for by subscription or by API key.
type splitAgent struct{ fakeAgent }

func (splitAgent) KeyEnv() string { return "PS_KEY" }
func (splitAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{agent.FeatureSignIn: agent.Yes}
}

func init() {
	agent.Register(splitAgent{fakeAgent{kind: "ps"}})
	agent.Register(rider{fakeAgent{kind: "ps-pb"}, "ps", "pb"})
	agent.Recheck()
}

// A split provider is two: its subscription runs only in its own harness,
// its key in any, each with its own default harness; an older config that
// ran it in another harness now runs its key there.
func TestSplitByBilling(t *testing.T) {
	if !equal(kinds(agent.RunsFor("ps")), []string{"ps"}) || !equal(kinds(agent.RunsFor("ps-key")), []string{"ps", "ps-pb"}) {
		t.Fatalf("sub %v, key %v", agent.RunsFor("ps"), agent.RunsFor("ps-key"))
	}
	c := Config{DefaultProfile: "ps", RunsIn: map[string]string{"ps": "pb"}, OldFolderRules: []FolderRule{{Path: "/w", Profile: "ps"}}}
	c.migrateKeyHarness()
	if c.DefaultProfile != "ps-key" || c.RunsIn["ps-key"] != "pb" || c.RunsIn["ps"] != "" || c.OldFolderRules[0].Profile != "ps-key" {
		t.Fatalf("migrated %+v", c)
	}
	key, ok := c.ProfileNamed("ps-key")
	if !ok || key.Billing != BillingKey || !equal(key.Kinds(), []string{"ps-pb"}) {
		t.Fatalf("key profile %+v kinds %v", key, key.Kinds())
	}
	if sub, _ := c.ProfileNamed("ps"); !equal(sub.Kinds(), []string{"ps"}) {
		t.Fatalf("sub runs %v", sub.Kinds())
	}
	c.SetUses("ps-key", "pb", true)
	c.SetUses("ps-key", "ps", true)
	c.SetUses("ps-key", "ps", false)
	if !c.Uses("ps-key", "pb") || c.Uses("ps-key", "ps") || c.Uses("ps", "pb") {
		t.Fatalf("used %v", c.Harnesses)
	}
}

func kinds(ks []agent.Kind) []string {
	out := make([]string, len(ks))
	for i, k := range ks {
		out[i] = string(k)
	}
	return out
}
