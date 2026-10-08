package state

import (
	"os"
	"slices"

	"github.com/0xdeafcafe/photon/uithread"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// A provider's API key pays for its models per token, apart from a
// subscription. rush keeps it in the vault; the config says only which
// providers have one, so drawing never reads the keychain.

func apiKeyID(p string) string { return "apikey-" + p }

// APIKey is provider p's API key: the one rush keeps, else the one in its
// variable. It reads the keychain: never on the UI thread.
func APIKey(p string) string {
	uithread.Forbid("state.APIKey")
	if b, err := Vault().Get(apiKeyID(p)); err == nil && len(b) > 0 {
		return string(b)
	}
	if e := agent.KeyEnv(p); e != "" {
		return os.Getenv(e)
	}
	return ""
}

// PutAPIKey keeps key as provider p's, or forgets p's when it's "". It
// writes the keychain: never on the UI thread. MarkAPIKey says so after.
func PutAPIKey(p, key string) error {
	uithread.Forbid("state.PutAPIKey")
	if key == "" {
		_ = Vault().Forget(apiKeyID(p))
		return nil
	}
	return Vault().Put(apiKeyID(p), []byte(key))
}

// MarkAPIKey says whether rush keeps an API key for provider p.
func (c *Config) MarkAPIKey(p string, has bool) {
	c.APIKeys = slices.DeleteFunc(c.APIKeys, func(x string) bool { return x == p })
	if has {
		c.APIKeys = append(c.APIKeys, p)
	}
}

// HasAPIKey is whether provider p has an API key: one rush keeps, or one
// in its variable.
func (c *Config) HasAPIKey(p string) bool {
	if slices.Contains(c.APIKeys, p) {
		return true
	}
	e := agent.KeyEnv(p)
	return e != "" && os.Getenv(e) != ""
}
