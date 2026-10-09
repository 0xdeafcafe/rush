package codex

import "testing"

func TestLocalPath(t *testing.T) {
	for in, want := range map[string]string{
		"file:///tmp/a%20b/Dark.png": "/tmp/a b/Dark.png",
		"/tmp/Dark.png":              "/tmp/Dark.png",
		"shots/Dark.png":             "shots/Dark.png",
	} {
		if got := localPath(in); got != want {
			t.Errorf("localPath(%q) = %q, want %q", in, got, want)
		}
	}
}
