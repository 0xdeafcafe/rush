package convo

import "testing"

// A group's rows are its own; a step inside keeps its rows, inside the
// group's; the gap after them is no one's.
func TestOwnerNestsInsideTheGroup(t *testing.T) {
	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: 80, Open: map[string]bool{}}, cw: 80}
	d.railed(2, true, func() {
		d.add("run", "", " ▾ hide 2 steps", "")
		from := len(d.lines)
		d.add("step", "", "  $ ls", "")
		d.add("", "", "    a.go", "")
		d.own("step", from)
		d.add("", "", "  loose", "")
	})
	d.add("", "", " ", "")
	d.own("other", len(d.lines)-1)
	want := []string{"run", "run\x00step", "run\x00step", "run", ""}
	for i, l := range d.lines {
		if l.Owner != want[i] {
			t.Errorf("row %d owner %q, want %q", i, l.Owner, want[i])
		}
	}
}
