package vibe

import (
	"crypto/sha256"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/keychain"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var readStoredKey = func() []byte {
	for _, service := range []string{"ai.mistral.vibe", "vibe"} {
		if b, err := keychain.Read(service, keyEnv); err == nil && len(b) > 0 {
			return b
		}
	}
	return nil
}

// AccountSummary reads only Vibe's own local credential and identity cache. The
// secret is used to select its matching cache entry and is never returned.
func (Adapter) AccountSummary(p agent.Profile) agent.AccountSummary {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	summary := agent.AccountSummary{Status: "unknown", Endpoint: "https://api.mistral.ai", Note: "Rush could not read Mistral credentials from the environment, keychain, or Vibe config home. This does not establish that you are signed out."}
	key := []byte(os.Getenv(keyEnv))
	if len(key) > 0 {
		summary.Method = "API key · environment"
	} else {
		key = readStoredKey()
		if len(key) > 0 {
			summary.Method = "API key · system keychain"
		} else {
			if b, err := os.ReadFile(filepath.Join(dir, ".env")); err == nil {
				for _, line := range strings.Split(string(b), "\n") {
					value, ok := strings.CutPrefix(strings.TrimPrefix(strings.TrimSpace(line), "export "), keyEnv+"=")
					if ok {
						key = []byte(strings.Trim(strings.TrimSpace(value), `"'`))
						break
					}
				}
			}
			if len(key) > 0 {
				summary.Method = "API key · Vibe .env"
			}
		}
	}
	if len(key) == 0 {
		return summary
	}
	summary.SignedIn = true
	summary.Status = "credentials available"
	summary.Note = "Credentials found; account details appear after Vibe caches its account lookup."
	hash := fmt.Sprintf("%x", sha256.Sum256(key))[:32]
	clear(key)
	var cache map[string]struct {
		Stored  int64 `json:"stored_at_timestamp"`
		Payload struct {
			Plan     string `json:"plan_name"`
			Type     string `json:"plan_type"`
			Org      string `json:"organization_kind"`
			Customer string `json:"customer_id"`
			Base     string `json:"api_base"`
		} `json:"payload"`
	}
	b, err := os.ReadFile(filepath.Join(dir, "whoami_cache.json"))
	if err != nil || jsonx.Unmarshal(b, &cache) != nil {
		return summary
	}
	entry, ok := cache[hash]
	if !ok {
		return summary
	}
	if entry.Stored <= time.Now().Add(-6*time.Hour).Unix() {
		summary.Note = "Credentials found; cached account details expired. Open Vibe once to refresh them."
		return summary
	}
	summary.Plan = entry.Payload.Plan
	if summary.Plan == "" {
		summary.Plan = entry.Payload.Type
	}
	summary.Org = entry.Payload.Org
	if entry.Payload.Customer != "" {
		summary.Name = "Customer " + entry.Payload.Customer
	}
	if u, err := url.Parse(entry.Payload.Base); err == nil && u.Host != "" && (u.Scheme == "https" || u.Scheme == "http") {
		summary.Endpoint = u.Scheme + "://" + u.Host
	}
	summary.Note = "From Vibe's local account cache; Vibe does not expose email in this record."
	return summary
}

var _ agent.AccountSummaryReader = Adapter{}
