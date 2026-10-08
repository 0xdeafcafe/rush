package host

import (
	"bytes"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// config.json keeps the profile as older rushes wrote and read it.
func TestConfigAccountOnDisk(t *testing.T) {
	old := []byte(`{"id":"abc","sessionId":"s","account":{"name":"work","configDir":"/h/.claude-work"},"cwd":"/w","kind":"codex"}`)
	var cfg Config
	if err := jsonx.Unmarshal(old, &cfg); err != nil {
		t.Fatal(err)
	}
	want := agent.Profile{Kind: "codex", Name: "work", Dir: "/h/.claude-work"}
	if cfg.Account != want || cfg.ID != "abc" || cfg.Cwd != "/w" {
		t.Fatalf("read %+v, want account %+v", cfg, want)
	}
	b, err := jsonx.Marshal(cfg)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(b, []byte(`"account":{"name":"work","configDir":"/h/.claude-work"}`)) || bytes.Contains(b, []byte(`"dir"`)) {
		t.Fatalf("wrote %s: the account must keep its old shape", b)
	}
	var back Config
	if err := jsonx.Unmarshal(b, &back); err != nil || back.Account != want {
		t.Fatalf("read back %+v, %v", back.Account, err)
	}
}
