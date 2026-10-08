package convo

import (
	"fmt"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// PlainText is the conversation as plain text for another model to
// summarise: your messages and its words in full, each tool call on a
// line with the start of what came back. Thinking is left out.
func (s *Session) PlainText() string {
	return s.plainText(func(int, *Turn) (int, int) { return 300, 400 })
}

// pruneRecent is how many of the latest turns pruning never touches.
const pruneRecent = 2

// PruneSteps are what a decision model judges to prune the conversation:
// each tool step before the latest turns as a line, and the work it's
// judged against: your latest ask, its latest words, your asks before.
func (s *Session) PruneSteps() (goal string, steps []string) {
	var asks []string
	said := ""
	for i := len(s.Turns) - 1; i >= 0 && len(asks) < 3; i-- {
		t := s.Turns[i]
		for j := len(t.Items) - 1; j >= 0 && said == ""; j-- {
			if t.Items[j].Kind == KText {
				said = clipText(t.Items[j].Text, 500)
			}
		}
		if t.Prompt != "" {
			asks = append(asks, clipText(t.Prompt, 500))
		}
	}
	var g strings.Builder
	for i, a := range asks {
		fmt.Fprintf(&g, "User:\n%s\n\n", a)
		if i == 0 && said != "" {
			fmt.Fprintf(&g, "Assistant, latest:\n%s\n\n", said)
		}
	}
	for i := range s.Turns[:max(0, len(s.Turns)-pruneRecent)] {
		for _, it := range s.Turns[i].Items {
			if it.Kind == KStep && it.Step != nil {
				st := it.Step
				steps = append(steps, st.Tool+" "+clipText(string(st.Input), 400)+"\n→ "+clipText(st.Output, 700))
			}
		}
	}
	return strings.TrimSpace(g.String()), steps
}

// Pruned is the conversation as PlainText has it, but each step of
// PruneSteps kept (keep[i]) in full and the rest cut to the call alone,
// and the latest turns' steps in full.
func (s *Session) Pruned(keep []bool) string {
	n, old := 0, len(s.Turns)-pruneRecent
	return s.plainText(func(turn int, _ *Turn) (int, int) {
		if turn >= old {
			return 2000, 4000
		}
		k := n < len(keep) && keep[n]
		n++
		if k {
			return 2000, 4000
		}
		return 120, 0
	})
}

func clipText(t string, n int) string {
	t = strings.TrimSpace(t)
	if len(t) > n {
		return strings.ToValidUTF8(t[:n], "") + " …"
	}
	return t
}

// plainText writes the conversation with each step's input and output
// cut to what clip says for it (an output of 0 is left out).
func (s *Session) plainText(clip func(turn int, t *Turn) (in, out int)) string {
	var b strings.Builder
	for ti, t := range s.Turns {
		switch {
		case t.Prompt != "":
			fmt.Fprintf(&b, "\n## User\n%s\n", t.Prompt)
		case t.From != "":
			fmt.Fprintf(&b, "\n## %s\n", t.From)
		}
		for _, it := range t.Items {
			switch it.Kind {
			case KText:
				fmt.Fprintf(&b, "\n## Assistant\n%s\n", it.Text)
			case KInterject:
				fmt.Fprintf(&b, "\n## User (mid-turn)\n%s\n", it.Text)
			case KCompact:
				fmt.Fprintf(&b, "\n## Summary of the conversation before this\n%s\n", it.Text)
			case KStep:
				if st := it.Step; st != nil {
					in, outN := clip(ti, t)
					fmt.Fprintf(&b, "- %s %s", st.Tool, clipText(string(st.Input), in))
					if out := clipText(st.Output, outN); outN > 0 && out != "" {
						fmt.Fprintf(&b, "\n  → %s", strings.ReplaceAll(out, "\n", "\n    "))
					} else if outN == 0 {
						b.WriteString(" (output dropped)")
					}
					b.WriteByte('\n')
				}
			}
		}
	}
	return strings.TrimSpace(b.String())
}

// CompactSystem is what a model compacting a session is told.
const CompactSystem = `You compact a coding agent's conversation so it can carry on in a fresh one with only your summary.
Write the summary the agent will read as its whole memory of the work. Keep, in this order:
1. What the user asked for, in their words where it matters, and every correction or preference they gave.
2. What was done: files changed and how, commands that mattered, decisions and why.
3. What is in progress right now and exactly what was about to happen next.
4. Open questions, errors not yet fixed, things the user is still waiting on.
Be specific (paths, names, numbers). Leave out chatter and anything finished that no longer matters. Write it as plain markdown, no preamble.`

// CompactedPrompt is the fresh conversation's first message.
func CompactedPrompt(by, summary string) string {
	return compactedHead + by + compactedMid + summary + compactedTail
}

// PrunedPrompt is the fresh conversation's first message when a decision
// model pruned it: the conversation itself, the steps it judged done cut
// to their calls.
func PrunedPrompt(by, transcript string) string {
	return "This conversation was pruned by " + by + " to make room: tool steps judged finished are cut to their calls, the rest is word for word. Here it is:\n\n" + transcript + compactedTail
}

const (
	compactedHead = "This conversation was compacted by "
	compactedMid  = " to make room. Here is the summary of everything before this point:\n\n"
	compactedTail = "\n\nTake this as what you know of the work so far. Reply only \"ready\" and wait for my next message."
)

// Compacting is whether a compaction is under way.
func (s *Session) Compacting() bool { return !s.compacting.IsZero() }

// MarkCompacting says rush began compacting it at at, or, given the zero
// time, that it's done: the working line shows it as Claude Code's own.
func (s *Session) MarkCompacting(at time.Time) { s.compacting = at }

// compactAsk draws a turn that asks for a compaction as rush's, not a
// message of yours: /compact, or the summary a fresh conversation starts
// from, which becomes its compaction divider (ctrl+o shows it).
func compactAsk(t *Turn) {
	p := strings.TrimSpace(t.Prompt)
	if p == "/compact" || strings.HasPrefix(p, "/compact ") {
		t.Prompt, t.Cause = "", p
		return
	}
	rest, ok := strings.CutPrefix(p, compactedHead)
	by, rest, ok2 := strings.Cut(rest, compactedMid)
	summary, _, _ := strings.Cut(rest, compactedTail)
	if !ok || !ok2 {
		return
	}
	t.Prompt, t.Cause = "", "carried on from a summary"
	t.Items = append(t.Items, &Item{Kind: KCompact, Text: summary, Compact: &event.Compacted{Trigger: "by " + by}})
}
