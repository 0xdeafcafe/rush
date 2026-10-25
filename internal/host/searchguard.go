package host

import (
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/bundled/searchguard"
	"github.com/0xdeafcafe/rush/internal/gate"
)

// searchRefusal is why the search guard refuses a shell call an agent
// asked approval for, or "". Claude Code's are refused in rush gate hook,
// before they're asked; Codex's and ACP agents' only reach rush here.
func searchRefusal(c *tool.Call) string {
	if c.Kind != tool.Shell || c.Input.Command == "" {
		return ""
	}
	return searchguard.Refusal(gate.Unwrap(c.Input.Command))
}
