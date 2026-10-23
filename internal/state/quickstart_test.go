package state

import (
	"encoding/json"
	"testing"
)

// Quick start is on until it's turned off, and turning it off sticks.
func TestQuickStartIsOnUnlessTurnedOff(t *testing.T) {
	for _, tc := range []struct {
		json string
		want bool
	}{{`{}`, true}, {`{"lean":true}`, true}, {`{"lean":false}`, false}} {
		var d Dispatch
		if err := json.Unmarshal([]byte(tc.json), &d); err != nil {
			t.Fatal(err)
		}
		if d.QuickStart() != tc.want {
			t.Errorf("%s: quick start %v", tc.json, d.QuickStart())
		}
	}
	off := false
	b, _ := json.Marshal(Dispatch{Lean: &off})
	var d Dispatch
	_ = json.Unmarshal(b, &d)
	if d.QuickStart() {
		t.Fatalf("turned off, saved as %s, read back on", b)
	}
}
