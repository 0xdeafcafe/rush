package host

import "testing"

func TestNameFrom(t *testing.T) {
	for _, c := range []struct{ in, want string }{
		{"fix the login page please", "fix the login page please"},
		{"look at this <pasted_content id=\"a\">\nlong log here\n</pasted_content id=\"a\"> [Image #1]", "look at this"},
		{"<pasted_content id=\"18da\">\nThe feed is definitely better than it used to be\n</pasted_content id=\"18da\">", "The feed is definitely better than"},
		{"[Image #1]", ""},
	} {
		if got := NameFrom(c.in); got != c.want {
			t.Errorf("NameFrom(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}
