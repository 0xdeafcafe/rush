package claude

import (
	"bytes"
	"context"
	"errors"
	"os/exec"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var _ agent.Summarizer = Adapter{}

// SummaryModels are Claude's, the cheapest first.
func (Adapter) SummaryModels(context.Context) ([]string, error) {
	return []string{"haiku", "sonnet", "opus"}, nil
}

// Summarize is claude -p with no tools, as the login in use.
func (a Adapter) Summarize(ctx context.Context, model, system, text string) (string, error) {
	prog := agent.Path(Kind)
	if prog == "" {
		return "", errors.New("claude isn't installed")
	}
	cmd := exec.CommandContext(ctx, prog, "-p", "--model", model, "--output-format", "json",
		"--no-session-persistence", "--strict-mcp-config", "--tools", "", "--system-prompt", system)
	cmd.Env = a.runAs().Env()
	cmd.Stdin = strings.NewReader(agent.FitTokens(text, 150_000))
	var out, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &stderr
	runErr := cmd.Run()
	var r struct {
		IsError bool   `json:"is_error"`
		Result  string `json:"result"`
	}
	switch {
	case jsonx.Unmarshal(out.Bytes(), &r) != nil:
		if runErr != nil {
			return "", errors.New(strings.TrimSpace(runErr.Error() + ": " + stderr.String()))
		}
		return "", errors.New("its answer couldn't be read")
	case r.IsError:
		return "", errors.New(r.Result)
	}
	return strings.TrimSpace(r.Result), nil
}
