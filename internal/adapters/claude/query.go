package claude

import (
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

var _ agent.Querier = Adapter{}

// QueryCommand is claude -p answering q alone: no shell, no web, no
// settings or hooks of yours, no MCP servers, file tools kept to q.Dirs,
// and no transcript kept, so its own runs don't show in what it reads.
func (a Adapter) QueryCommand(p agent.Profile, q agent.Query) (args, env []string) {
	args = make([]string, 0, 24+2*len(q.Dirs))
	args = append(args, "-p",
		"--model", q.Model,
		"--output-format", "json",
		"--no-session-persistence",
		"--restricted", "--strict-mcp-config",
		"--tools", "Read,Grep,Glob",
		"--permission-mode", "dontAsk", "--permission-prompts", "none",
		"--system-prompt", q.System,
		"--max-budget-usd", fmt.Sprintf("%.2f", q.Budget),
		"--json-schema", q.Schema,
	)
	allow := make([]string, 0, 3*len(q.Dirs))
	for _, d := range q.Dirs {
		args = append(args, "--add-dir", d)
		for _, t := range []string{"Read", "Grep", "Glob"} {
			allow = append(allow, t+"("+d+"/**)")
		}
	}
	args = append(args, "--allowedTools", strings.Join(allow, ","))
	as := Account(p)
	if as.IsDefault() {
		as = a.runAs() // the login in use, in its home
	}
	return args, as.Env()
}

// QueryAnswer reads claude -p's JSON result.
func (Adapter) QueryAnswer(stdout []byte) (agent.Answer, bool) {
	var r struct {
		IsError    bool           `json:"is_error"`
		Subtype    string         `json:"subtype"`
		Result     string         `json:"result"`
		Cost       float64        `json:"total_cost_usd"`
		Structured jsontext.Value `json:"structured_output"`
	}
	if jsonx.Unmarshal(stdout, &r) != nil {
		return agent.Answer{}, false
	}
	a := agent.Answer{Out: r.Structured, Cost: r.Cost}
	if r.IsError {
		first, _, _ := strings.Cut(strings.TrimSpace(r.Result), "\n")
		a.Err = errors.New(r.Subtype + ": " + first)
	}
	return a, true
}

// QueryModels are Haiku and Opus.
func (Adapter) QueryModels() (quick, careful string) { return "haiku", "opus" }
