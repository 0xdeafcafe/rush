package state

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// A harness switched off runs nothing: a provider whose ★ it was goes to
// its first other cell that's on, and comes back once it's on again.
func TestHarnessOff(t *testing.T) {
	c := Config{}
	if !c.CellOn("pa", "pa") || c.CellOn("pa", "pb") || c.HarnessFor("pa") != "pa" {
		t.Fatalf("fresh: ★ on, the rest off, auto in its own")
	}
	c.SetUses("pa", "pb", true)
	c.SetHarnessOn("pa", false)
	if c.CellOn("pa", "pa") || c.HarnessFor("pa") != "pb" {
		t.Fatalf("pa off: auto in %s", c.HarnessFor("pa"))
	}
	if p, _ := c.ProfileNamed("pa"); !equal(p.Kinds(), []string{"pa-pb"}) {
		t.Fatalf("pa's profile runs %v", p.Kinds())
	}
	c.SetHarnessOn("pa", true)
	if c.HarnessFor("pa") != "pa" || len(c.HarnessesOff) != 0 {
		t.Fatalf("back on: auto in %s, off %v", c.HarnessFor("pa"), c.HarnessesOff)
	}
}

// A cell calls every model until some are picked; its default always.
func TestCellModels(t *testing.T) {
	c := Config{}
	if !c.Calls("pa", "pb", "m1") {
		t.Fatal("no list is every model")
	}
	c.SetCellModels("pa", "pb", []string{"m2"})
	if c.Calls("pa", "pb", "m1") || !c.Calls("pa", "pb", "m2") || !c.Calls("pa", "pb", "") || !c.Calls("pa", "pa", "m1") {
		t.Fatal("picked m2 in pa@pb alone")
	}
	c.SetCellModels("pa", "pb", nil)
	if len(c.CellModels) != 0 {
		t.Fatalf("none picked is all again: %v", c.CellModels)
	}
}

// A harness's own effort wins over every harness's; a route's own only
// counts until they were moved onto harnesses.
func TestResolved(t *testing.T) {
	d := Dispatch{Every: Start{Effort: "high"}}
	d.SetHarnessStart("pb", Start{Effort: "low", Model: "ignored"})
	d.SetStartOn("pa", "pa", Start{Model: "m1", Effort: "max"})
	if st := d.Resolved("pa", "pa-pb"); st.Effort != "low" || st.Model != "" {
		t.Fatalf("pa in pb: %+v", st)
	}
	if st := d.Resolved("pa", "pa"); st.Effort != "high" || st.Model != "m1" {
		t.Fatalf("pa: %+v", st)
	}
	d.Every = Start{}
	if st := d.Resolved("pa", "pa"); st.Effort != "max" {
		t.Fatalf("before the move, the route's: %+v", st)
	}
	d.moveHarnessStarts()
	if st := d.Resolved("pa", "pa"); st.Effort != "max" || d.HarnessStart("pa").Effort != "max" {
		t.Fatalf("moved onto pa: %+v", d.ByHarness)
	}
	if ApproveAllMode([]agent.Choice{{ID: "default"}, {ID: "full-access"}}) != "full-access" {
		t.Fatal("approve everything is the harness's own mode for it")
	}
}
