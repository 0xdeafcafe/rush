package codex

import "testing"

func TestModeOfPolicy(t *testing.T) {
	for want, m := range modes {
		if got := modeOf(m.approval, m.sandboxTurn["type"].(string)); got != want {
			t.Errorf("modeOf(%s) = %q", want, got)
		}
	}
	if got := modeOf("on-request", "somethingNew"); got != "" {
		t.Errorf("unknown policy read as %q", got)
	}
}
