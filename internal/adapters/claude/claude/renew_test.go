package claude

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// tokenServer stands in for Claude Code's token endpoint, answering with
// status and body; asked counts the requests.
func tokenServer(t *testing.T, status int, body string) (asked *int) {
	t.Helper()
	asked = new(int)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		*asked++
		var got map[string]string
		if err := jsonx.Decode(r.Body, &got); err != nil || r.Method != "POST" || r.Header.Get("Content-Type") != "application/json" {
			t.Errorf("request: %s %v %v", r.Method, got, err)
		}
		want := map[string]string{"grant_type": "refresh_token", "refresh_token": "fake-refresh-1", "client_id": clientID, "scope": "user:profile user:inference"}
		for k, v := range want {
			if got[k] != v {
				t.Errorf("%s = %q, want %q", k, got[k], v)
			}
		}
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(srv.Close)
	old := tokenURL
	tokenURL = srv.URL
	t.Cleanup(func() { tokenURL = old })
	return asked
}

const fakeCred = `{"claudeAiOauth":{"accessToken":"fake-access-1","refreshToken":"fake-refresh-1","expiresAt":1,"scopes":["user:profile","user:inference"],"subscriptionType":"max"},"mcpOAuth":{"x":{"accessToken":"fake-mcp"}}}`

// memStore is a sign-in kept in memory.
func memStore(cred string) (read func() ([]byte, error), write func([]byte) error, got *string) {
	got = &cred
	return func() ([]byte, error) { return []byte(*got), nil }, func(b []byte) error { *got = string(b); return nil }, got
}

// The refresh token rotates: the new one, the new access token and its
// expiry are written back, every other field as it was.
func TestRenewWritesRotatedTokens(t *testing.T) {
	tokenServer(t, 200, `{"access_token":"fake-access-2","refresh_token":"fake-refresh-2","expires_in":3600}`)
	read, write, got := memStore(fakeCred)
	if err := renew(context.Background(), read, write); err != nil {
		t.Fatal(err)
	}
	var c struct {
		OAuth struct {
			Access    string   `json:"accessToken"`
			Refresh   string   `json:"refreshToken"`
			Sub       string   `json:"subscriptionType"`
			ExpiresAt int64    `json:"expiresAt"`
			Scopes    []string `json:"scopes"`
		} `json:"claudeAiOauth"`
		MCP map[string]map[string]string `json:"mcpOAuth"`
	}
	if err := jsonx.Unmarshal([]byte(*got), &c); err != nil {
		t.Fatal(err)
	}
	o := c.OAuth
	if o.Access != "fake-access-2" || o.Refresh != "fake-refresh-2" || o.Sub != "max" || len(o.Scopes) != 2 || c.MCP["x"]["accessToken"] != "fake-mcp" {
		t.Fatalf("written back: %s", *got)
	}
	if left := time.Until(time.UnixMilli(o.ExpiresAt)); left < 59*time.Minute || left > time.Hour {
		t.Fatalf("expires in %s", left)
	}
}

func TestRenewRevoked(t *testing.T) {
	tokenServer(t, 400, `{"error":"invalid_grant","error_description":"Refresh token not found or invalid"}`)
	read, write, got := memStore(fakeCred)
	if err := renew(context.Background(), read, write); !errors.Is(err, ErrRevoked) || *got != fakeCred {
		t.Fatalf("err %v, stored %s", err, *got)
	}
}

// A Claude Code running on the folder refreshes it itself: rush doesn't.
func TestRenewRefusesFolderInUse(t *testing.T) {
	asked := tokenServer(t, 200, `{}`)
	old := InUse
	InUse = func(Account) bool { return true }
	defer func() { InUse = old }()
	if err := Renew(context.Background(), Account{ConfigDir: t.TempDir()}); !errors.Is(err, ErrInUse) || *asked != 0 {
		t.Fatalf("err %v, asked %d", err, *asked)
	}
}

// An expired reading refreshes the sign-in once, not again for ten minutes.
func TestOrRenewOnceInTenMinutes(t *testing.T) {
	renews := 0
	fetch := func() (Usage, error) { return Usage{}, ErrExpired }
	for range 2 {
		_, _ = orRenew("login:test-once", fetch, func() error { renews++; return nil })
	}
	if renews != 1 {
		t.Fatalf("renewed %d times", renews)
	}
}
