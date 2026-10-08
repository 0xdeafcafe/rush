package codex

import (
	"encoding/base64"
	"os"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// An auth.json says who it signs in as, from its id token, and an API
// key's account is named without the key.
func TestWhoIs(t *testing.T) {
	claims, _ := jsonx.Marshal(map[string]any{"email": "me@example.com", "https://api.openai.com/auth": map[string]any{"chatgpt_plan_type": "plus"}})
	tok := "e30." + base64.RawURLEncoding.EncodeToString(claims) + ".sig"
	b, _ := jsonx.Marshal(map[string]any{"auth_mode": "chatgpt", "tokens": map[string]any{"id_token": tok, "account_id": "acct-1", "refresh_token": "secret"}})
	a, err := whoIs(b)
	if err != nil || a.ID != "acct-1" || a.Email != "me@example.com" || a.Plan != "plus" || a.Key != "codex:acct-1" {
		t.Fatalf("got %+v, %v", a, err)
	}
	b, _ = jsonx.Marshal(map[string]any{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-secret"})
	a, err = whoIs(b)
	if err != nil || !strings.HasPrefix(a.ID, "apikey-") || strings.Contains(a.ID+a.Name, "secret") {
		t.Fatalf("got %+v, %v", a, err)
	}
	if _, err := whoIs([]byte(`{"auth_mode":"chatgpt"}`)); err == nil {
		t.Fatal("an empty auth.json signs in as someone")
	}
}

// The machine's own sign-in, read but never shown.
func TestWhoIsHere(t *testing.T) {
	home, _ := os.UserHomeDir()
	b, err := os.ReadFile(home + "/.codex/auth.json")
	if err != nil {
		t.Skip("no Codex sign-in here")
	}
	a, err := whoIs(b)
	if err != nil || a.ID == "" {
		t.Fatalf("this machine's sign-in: %v", err)
	}
	t.Logf("signed in, plan %q, email known %v", a.Plan, a.Email != "")
}
