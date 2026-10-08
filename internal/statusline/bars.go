package statusline

import (
	"os"
	"path/filepath"
	"reflect"
	"slices"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Bars are rush's own status lines, built the same way as Claude Code's
// but drawn by rush from what it knows: Top at the top right of the
// window, about every agent at once, and Agent at the top of an agent's
// Session. Their segments are rush's (see ui/bars.go).
type Bars struct {
	Top   Layout `json:"top"`
	Agent Layout `json:"agent"`
}

// BarLines is how many lines the agent header has room for; TopLines,
// rush's top, has one more, beside the tabs.
const (
	BarLines = 2
	TopLines = 3
)

// DefaultTop keeps everyday status quiet; detailed segments remain available
// in the status-line editor.
func DefaultTop() Layout {
	return Layout{Lines: [][]string{{"system", "today", "plan", "memory", "version"}, {}}, Sep: " · "}
}

// oldTops are exact historical defaults, including the full metric strip.
// Only an unchanged default and separator migrate to the quieter layout.
var oldTops = [][][]string{
	{{"today", "plan"}, {"memory", "system", "statushelp"}},
	{{"today", "usage"}, {"ram", "tokens", "cpu", "net", "disk", "battery", "tmp"}},
	{{"today", "usage"}, {"ram", "cpu", "tmp"}},
	{{"today", "usage"}, {"ram", "cpu", "disk", "battery", "tmp"}},
	{{"today", "usage"}, {"ram", "cpu", "net", "disk", "battery", "tmp"}},
}

// oldAgents are exact historical agent defaults.
var oldAgents = [][][]string{
	{{"context", "cost", "billing"}, {"folder", "branch", "model", "effort", "mode", "tmp"}},
	{{"context", "cost"}, {"folder", "branch", "model", "effort", "mode", "tmp"}},
}

func DefaultAgent() Layout {
	return Layout{Lines: [][]string{{"context", "model", "effort", "billing", "rush"}, {"folder", "branch"}}, Sep: " · "}
}

// BarsPath is where they're kept.
func BarsPath() string { return filepath.Join(state.Dir(), "bars.json") }

// LoadBars reads them; one never saved is its default.
func LoadBars() Bars {
	b := Bars{Top: DefaultTop(), Agent: DefaultAgent()}
	raw, err := os.ReadFile(BarsPath())
	if err != nil {
		return b
	}
	var got Bars
	if jsonx.Unmarshal(raw, &got) != nil {
		return b
	}
	if got.Top.Lines != nil && !(slices.ContainsFunc(oldTops, func(o [][]string) bool { return reflect.DeepEqual(got.Top.Lines, o) }) && got.Top.Sep == b.Top.Sep) {
		b.Top = got.Top
	}
	if got.Agent.Lines != nil && !(slices.ContainsFunc(oldAgents, func(o [][]string) bool { return reflect.DeepEqual(got.Agent.Lines, o) }) && got.Agent.Sep == b.Agent.Sep) {
		b.Agent = got.Agent
	}
	return b
}

// SaveBars writes them.
func SaveBars(b Bars) error {
	raw, err := jsonx.MarshalIndent(b)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(BarsPath()), 0o700); err != nil {
		return err
	}
	return os.WriteFile(BarsPath(), append(raw, '\n'), 0o600)
}
