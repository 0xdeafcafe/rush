package failure

import "testing"

func TestKindOf(t *testing.T) {
	for _, c := range []struct {
		err  string
		want Kind
	}{
		// Usage limits, as host.isLimit read them.
		{"Claude usage limit reached", Limit},
		{"You've hit your session limit · resets 5am", Limit},
		{"You've hit your weekly limit", Limit},
		{"usage_limit_reached", Limit},
		// Signed out: what isAuthError read, and what it missed.
		{"Not logged in · Please run /login", Auth},
		{"API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\"}}", Auth},
		{"OAuth token has expired. Please obtain a new token", Auth},
		{"Invalid API key · Please run /login", Auth},
		{"apiKeyHelper failed", Auth},
		{"API Error: 403 {\"error\":{\"type\":\"permission_error\"}}", Auth},
		{"invalid_grant: refresh token revoked", Auth},
		{"unexpected status 401 Unauthorized", Auth},
		// Busy or rate limited, never the login.
		{"API Error: 529 Overloaded", Capacity},
		{"API Error: 429 {\"type\":\"rate_limit_error\"}", Capacity},
		{"exceeded retry limit, last status: 429 Too Many Requests", Capacity},
		{"insufficient_quota", Capacity},
		// The network, as IsOffline read it.
		{"API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)", Offline},
		{"API Error: Unable to connect to API (ENOTFOUND)", Offline},
		{"API Error: No response from API (waited 5m, then 10m on the retry).", Offline},
		{"API Error: Your computer went to sleep mid-response.", Offline},
		// Failing for a moment, as IsRetryable read it.
		{"API Error: Connection lost mid-response.", Transient},
		{"API Error: Response stalled mid-stream.", Transient},
		{"API Error: Connection error.", Transient},
		{"API Error: 500 Internal server error", Transient},
		{"API Error: 503 service unavailable", Transient},
		{"Request timed out", Transient},
		// Nothing about the provider, and words too broad to count.
		{"Prompt is too long", Fatal},
		{"error_during_execution", Fatal},
		{"failed to login to the MCP server's dashboard", Fatal},
		{"used 4013 tokens", Fatal},
		{"", Fatal},
	} {
		if got := KindOf(c.err); got != c.want {
			t.Errorf("KindOf(%q) = %v, want %v", c.err, got, c.want)
		}
	}
}

func TestSummary(t *testing.T) {
	for err, want := range map[string]string{
		"API Error: 401 {\"error\":{\"type\":\"authentication_error\",\"message\":\"sk-ant-oat01-abcdef invalid\"}}": "401 authentication_error",
		"unexpected status 401 Unauthorized: Bearer eyJhbGciOi":                                                      "401 unauthorized",
		"exceeded retry limit, last status: 429 Too Many Requests":                                                   "429",
		"invalid_grant invalid_grant revoked expired forbidden unauthorized quota":                                   "invalid_grant revoked expired forbidden unauthorized",
		"connect ECONNREFUSED 127.0.0.1:443":                                                                         "econnrefused",
		"Not logged in · Please run /login":                                                                          "signed out",
		"Connection lost mid-response.":                                                                              "request failed",
		"my token is hunter2":                                                                                        "request failed",
		"the page said 404 twice":                                                                                    "request failed",
	} {
		if got := Summary(err); got != want {
			t.Errorf("Summary(%q) = %q, want %q", err, got, want)
		}
	}
}

func TestAdd(t *testing.T) {
	Add(Auth, "Refresh_Token_Reused")
	if KindOf("error: refresh_token_reused") != Auth || Summary("refresh_token_reused") != "refresh_token_reused" {
		t.Error("an adapter's word should sort and summarise")
	}
}
