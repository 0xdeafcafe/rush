package ghpr

import (
	"context"
	"testing"
)

func TestParseURL(t *testing.T) {
	for u, want := range map[string]struct {
		repo string
		n    int
		ok   bool
	}{
		"https://github.com/acme/terrafied/pull/33":          {"acme/terrafied", 33, true},
		"https://github.com/acme/terrafied/pull/33/files":    {"acme/terrafied", 33, true},
		"https://github.com/acme/terrafied/pull/33#issue-1":  {"acme/terrafied", 33, true},
		"https://www.github.com/acme/terrafied/pull/7":       {"acme/terrafied", 7, true},
		"https://github.com/acme/terrafied/issues/33":        {},
		"https://github.com/acme/terrafied/pull/x":           {},
		"https://github.com/acme/terrafied/pull/0":           {},
		"https://gitlab.com/acme/terrafied/pull/33":          {},
		"https://github.com/acme/terrafied":                  {},
		"file:///Users/me/github.com/acme/terrafied/pull/33": {},
	} {
		repo, n, ok := ParseURL(u)
		if repo != want.repo || n != want.n || ok != want.ok {
			t.Errorf("ParseURL(%q) = %q %d %v, want %+v", u, repo, n, ok, want)
		}
	}
	if repo, n, ok := ParseIssueURL("https://github.com/acme/terrafied/issues/12"); repo != "acme/terrafied" || n != 12 || !ok {
		t.Errorf("ParseIssueURL = %q %d %v", repo, n, ok)
	}
}

// View asks gh of the pull request by its address, from no folder in
// particular, and reads its one answer.
func TestView(t *testing.T) {
	was := RunGH
	t.Cleanup(func() { RunGH = was })
	var asked []string
	RunGH = func(_ context.Context, dir string, args ...string) ([]byte, error) {
		asked = append([]string{dir}, args...)
		return []byte(`{"number":33,"title":"Emoji fix","url":"https://github.com/acme/terrafied/pull/33","state":"OPEN",
		"statusCheckRollup":[{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"}]}`), nil
	}
	pr, err := View(context.Background(), "https://github.com/acme/terrafied/pull/33")
	if err != nil || pr.Number != 33 || pr.State != "OPEN" || len(pr.Checks) != 1 {
		t.Fatalf("View = %+v, %v", pr, err)
	}
	if len(asked) < 4 || asked[0] != "" || asked[1] != "pr" || asked[2] != "view" || asked[3] != "https://github.com/acme/terrafied/pull/33" {
		t.Errorf("asked gh %q", asked)
	}
}

func TestSel(t *testing.T) {
	if got := Sel(7, ""); got != "7" {
		t.Errorf("Sel by number = %q", got)
	}
	if got := Sel(7, "https://github.com/a/b/pull/7"); got != "https://github.com/a/b/pull/7" {
		t.Errorf("Sel by address = %q", got)
	}
}
