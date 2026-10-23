package host

import (
	"strings"
	"testing"
)

// A run that gave up tells the session it reports to the error, briefly
// and without secrets, its last step, the retries rush made and, for a
// login that stopped working, how to sign in again.
func TestFailReport(t *testing.T) {
	got := FailReport("claude", "API Error: 401 invalid_grant sk-ant-secret", "Bash (go test ./...)", 8)
	for _, want := range []string{"provider error: 401 invalid_grant", "Its last step: Bash (go test ./...)", "rush retried it 8 times", "claude auth login"} {
		if !strings.Contains(got, want) {
			t.Errorf("%q lacks %q", got, want)
		}
	}
	if strings.Contains(got, "sk-ant") {
		t.Errorf("a secret reached the report: %q", got)
	}
	if got := FailReport("claude", "Prompt is too long\nmore", "", 0); got != "Prompt is too long" {
		t.Errorf("an error not the provider's stays as it was said: %q", got)
	}
}

// The retry names the error it stopped on, and asks not to redo the work.
func TestRetryPrompt(t *testing.T) {
	got := RetryPrompt("API Error: 429 Too Many Requests")
	if got != "Your previous run stopped with a provider error (429). Continue from where you left off; check what you already did and do not redo it." {
		t.Fatalf("got %q", got)
	}
}
