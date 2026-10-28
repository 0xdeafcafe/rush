package agent

import "testing"

func TestTrustedAs(t *testing.T) {
	kimi := []Choice{{ID: "default"}, {ID: "plan"}, {ID: "auto"}, {ID: "yolo"}}
	gemini := []Choice{{ID: "default"}, {ID: "autoEdit"}, {ID: "yolo"}, {ID: "plan"}}
	for _, c := range []struct {
		mode  string
		modes []Choice
		want  string
	}{
		{"auto", kimi, "auto"},
		{"bypassPermissions", kimi, "yolo"},
		{"full-access", gemini, "yolo"},
		{"auto", gemini, "autoEdit"}, // never more than the starter had
		{"default", kimi, ""},
		{"plan", kimi, ""},
		{"", kimi, ""},
	} {
		if got := TrustedAs(c.mode, c.modes); got != c.want {
			t.Errorf("TrustedAs(%q) = %q, want %q", c.mode, got, c.want)
		}
	}
}
