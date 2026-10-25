package host

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

func TestSearchRefusal(t *testing.T) {
	for _, c := range []struct {
		name    string
		call    tool.Call
		refused bool
	}{
		{"codex recursive grep", tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "grep -rn foo ."}}, true},
		{"acp xargs git grep", tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "cat names | xargs -I{} git grep -n {}"}}, true},
		{"wrapped by the gate", tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "'rush' gate run --name 'grep' --dir '/x' -- bash -c 'grep -r foo .'"}}, true},
		{"fine search", tool.Call{Kind: tool.Shell, Input: tool.Input{Command: "rg -n foo"}}, false},
		{"not a shell call", tool.Call{Kind: tool.Edit, Input: tool.Input{Command: "grep -rn foo ."}}, false},
		{"no command", tool.Call{Kind: tool.Shell}, false},
	} {
		if got := searchRefusal(&c.call) != ""; got != c.refused {
			t.Errorf("%s: refused = %v, want %v", c.name, got, c.refused)
		}
	}
}
