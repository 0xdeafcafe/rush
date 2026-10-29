package agent

import (
	"context"
	"errors"
	"strings"
)

// AskCheap asks the cheapest model rush can run without a session, the
// first of its models whose name has prefer when one does, and says which
// it asked.
func AskCheap(ctx context.Context, system, prompt, prefer string) (string, string, error) {
	for _, k := range []Kind{LegacyKind, "ollama"} {
		a, ok := Get(k)
		sm, can := a.(Summarizer)
		if !ok || !can || !Installed(k) {
			continue
		}
		ms, err := sm.SummaryModels(ctx)
		if err != nil || len(ms) == 0 {
			continue
		}
		pick := ms[0]
		for _, x := range ms {
			if prefer != "" && strings.Contains(x, prefer) {
				pick = x
				break
			}
		}
		out, err := sm.Summarize(ctx, pick, system, prompt)
		return pick, out, err
	}
	return "", "", errors.New("no model to ask: it runs on Claude Code or Ollama")
}
