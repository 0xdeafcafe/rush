package main

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/bundled/searchguard"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// With search-guard on, the hook denies a recursive grep and says why; off,
// the call goes as it is.
func TestSearchGuardHook(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	call := `{"tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":"grep -rn foo ."}}`
	var out strings.Builder
	gateHook(strings.NewReader(call), &out)
	if out.Len() != 0 {
		t.Fatalf("off, the hook answered: %s", out.String())
	}
	if err := plugin.SetBundled(searchguard.Name, true); err != nil {
		t.Fatal(err)
	}
	gateHook(strings.NewReader(call), &out)
	if s := out.String(); !strings.Contains(s, `"permissionDecision":"deny"`) || !strings.Contains(s, "rg -n") {
		t.Fatalf("on, the hook let it through: %s", s)
	}
}
