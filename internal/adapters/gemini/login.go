package gemini

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// SignInCommand opens Gemini's native authentication picker. Exit Gemini after
// signing in to return to Rush; no model request is supplied by this command.
func (Adapter) SignInCommand(p agent.Profile) (*exec.Cmd, error) {
	bin := agent.Path(Kind)
	if bin == "" {
		return nil, fmt.Errorf("Gemini CLI is not installed")
	}
	cmd := exec.Command(bin)
	if p.Dir != "" && filepath.Base(p.Dir) == ".gemini" {
		cmd.Env = append(os.Environ(), "GEMINI_CLI_HOME="+filepath.Dir(p.Dir))
	}
	return cmd, nil
}

var _ agent.Authenticator = Adapter{}

// CheckKey checks credentials without starting Gemini or making an API request.
func (Adapter) CheckKey(p agent.Profile) error {
	if os.Getenv("GEMINI_API_KEY") != "" || os.Getenv("GOOGLE_API_KEY") != "" {
		return nil
	}
	if p.Dir == "" {
		p.Dir = geminiHome()
	}
	var token struct {
		Access  string `json:"access_token"`
		Refresh string `json:"refresh_token"`
	}
	if b, err := os.ReadFile(filepath.Join(p.Dir, credsFile)); err == nil && jsonx.Unmarshal(b, &token) == nil && (token.Access != "" || token.Refresh != "") {
		return nil
	}
	return fmt.Errorf("Gemini isn't signed in: open Sign in to use Google, or set GEMINI_API_KEY")
}
