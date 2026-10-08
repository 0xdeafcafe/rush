package pi

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// AccountSummary is the provider Pi runs on by default and how it's signed
// in to it, from auth.json, settings.json and the environment. Keys and
// tokens are only checked for, never returned.
func (Adapter) AccountSummary(p agent.Profile) agent.AccountSummary {
	if p.Dir == "" {
		p.Dir = Home()
	}
	var creds map[string]struct {
		Type  string `json:"type"`
		Email string `json:"email"`
	}
	if b, err := os.ReadFile(filepath.Join(p.Dir, "auth.json")); err == nil {
		_ = jsonx.Unmarshal(b, &creds)
	}
	var settings struct {
		Provider string `json:"defaultProvider"`
	}
	if b, err := os.ReadFile(filepath.Join(p.Dir, "settings.json")); err == nil {
		_ = jsonx.Unmarshal(b, &settings)
	}
	var stored []string
	for id := range creds {
		stored = append(stored, id)
	}
	sort.Strings(stored)
	s := agent.AccountSummary{Name: settings.Provider}
	if c, ok := creds[settings.Provider]; ok {
		s.SignedIn, s.Email = true, c.Email
		s.Method = "API key · auth.json"
		if c.Type == "oauth" {
			s.Method = "OAuth · auth.json"
		}
	} else if settings.Provider != "" && os.Getenv(envKey(settings.Provider)) != "" {
		s.SignedIn, s.Method = true, "API key · "+envKey(settings.Provider)
	}
	switch {
	case settings.Provider == "" && len(stored) > 0:
		s.SignedIn = true
		s.Note = "No default provider set; signed in to " + strings.Join(stored, ", ")
	case !s.SignedIn && settings.Provider != "":
		s.Note = "No credentials for " + settings.Provider + ": run /login in Pi or set " + envKey(settings.Provider)
	case !s.SignedIn:
		s.Note = "No credentials found: run /login in Pi, or set a provider's API key"
	case len(stored) > 1:
		s.Note = "Also signed in to " + strings.Join(stored, ", ")
	}
	return s
}

// envKey is the variable Pi reads a provider's key from, for the usual ones.
func envKey(provider string) string {
	if provider == "google" {
		return "GEMINI_API_KEY"
	}
	return strings.ToUpper(strings.ReplaceAll(provider, "-", "_")) + "_API_KEY"
}

// SignInCommand opens Pi itself: it signs in with /login, inside a session.
func (Adapter) SignInCommand(p agent.Profile) (*exec.Cmd, error) {
	bin := agent.Path(Kind)
	if bin == "" {
		return nil, errors.New("Pi is not installed")
	}
	cmd := exec.Command(bin)
	if p.Dir != "" && p.Dir != Home() {
		cmd.Env = append(os.Environ(), "PI_CODING_AGENT_DIR="+p.Dir)
	}
	return cmd, nil
}

var (
	_ agent.AccountSummaryReader = Adapter{}
	_ agent.Authenticator        = Adapter{}
)
