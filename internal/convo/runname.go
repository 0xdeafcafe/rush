package convo

import (
	"github.com/0xdeafcafe/rush/internal/agent"
)

// RunTag is what runs a subagent run, as every place that lists one says
// it after what it was asked: the model, or the agent type when the model
// isn't known. A run in a harness other than home, the session's own, says
// so first, with that harness's mark and name: "◎ Codex astra".
func RunTag(kind, home agent.Kind, model, typ string) string {
	if home == "" {
		home = agent.LegacyKind
	}
	tag := typ
	if tag == "subagent" { // only that it's one, which its row says already
		tag = ""
	}
	if model != "" && model[0] != '<' { // not <synthetic>
		k := kind
		if k == "" {
			k = home
		}
		tag = agent.ModelName(k, model)
	}
	if !OtherHarness(kind, home) || kind == home {
		return tag
	}
	h := agent.HarnessOf(kind)
	name := string(h)
	if a, ok := agent.Get(h); ok {
		name = a.Name()
	}
	mark := ""
	if f := peerMark.Load(); f != nil {
		mark = (*f)(string(kind)) + " "
	}
	if tag == "" || tag == name || tag == string(kind) {
		return mark + name
	}
	return mark + name + " " + tag
}

// OtherHarness is whether a run of kind works in another harness than
// home, the session that started it.
func OtherHarness(kind, home agent.Kind) bool {
	if home == "" {
		home = agent.LegacyKind
	}
	if kind == "" {
		return false
	}
	return agent.HarnessOf(kind) != agent.HarnessOf(home)
}
