package ui

import (
	"context"
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/adapters/ollama"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// #compact: compact a session with its own model, as /compact does, or
// have another write the summary (a cheaper Claude, a local Ollama model)
// and carry on in a fresh conversation that starts with it. The cache
// goes either way; the conversation left is a path for /rewind.

type summarizer struct {
	kind  agent.Kind
	label string // "Ollama", "Claude"
	model string // "" for the session's own /compact
	// prune has model, a decision model, drop the steps it judges done
	// and keep the rest word for word, in place of a summary.
	prune bool
}

type compactSheet struct {
	sess, id, name string
	opts           []summarizer
	errs           []string // summarisers that couldn't say their models
	loading        bool
	cur            int
	details        int
}

func (m *Model) openCompact(c *hostConn, a *fleet.Agent) tea.Cmd {
	if c == nil || c.client == nil {
		m.flash("#compact works on a session rush runs: open one first", true)
		return nil
	}
	kind := agent.Migrated(firstNonEmpty(c.sess.Info.Kind, a.Kind))
	s := &compactSheet{sess: c.key, id: a.ID, name: a.DisplayName, loading: true}
	if agent.Supports(kind, agent.FeatureCompact) {
		s.opts = append(s.opts, summarizer{label: "native harness compaction"})
	}
	m.sheet = s
	if !agent.Supports(kind, agent.FeatureRewind) {
		s.loading = false
		s.errs = append(s.errs, "External summaries are unavailable: this harness cannot restore the original conversation.")
		return nil
	}
	if c.sess.Info.Proto < 9 {
		s.loading = false
		s.errs = append(s.errs, "Restart this session’s host to enable guarded local compaction; native compaction remains available.")
		return nil
	}
	opened := s
	type found struct {
		opts []summarizer
		errs []string
	}
	return later(func() found {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		var opts []summarizer
		var errs []string
		for _, ad := range agent.InstalledAll() {
			sz, ok := ad.(agent.Summarizer)
			if !ok {
				continue
			}
			models, err := sz.SummaryModels(ctx)
			if err != nil {
				errs = append(errs, ad.Name()+": "+err.Error())
			}
			if x, ok := sz.(interface{ SummaryExclusions() []string }); ok {
				errs = append(errs, x.SummaryExclusions()...)
			}
			for _, md := range models {
				opts = append(opts, summarizer{kind: ad.Kind(), label: ad.Name(), model: md})
			}
			if ad.Kind() == ollama.Kind {
				decide, _ := ollama.DecisionModels(ctx)
				for _, md := range decide {
					opts = append(opts, pruner(md))
				}
			}
		}
		return found{opts, errs}
	}, func(m *Model, f found) tea.Cmd {
		s, ok := m.sheet.(*compactSheet)
		if !ok || s != opened {
			return nil
		}
		s.opts, s.errs, s.loading = append(s.opts, f.opts...), f.errs, false
		found := false
		for i, o := range s.opts {
			if isDefault(m, o) {
				s.cur = i
				found = true
			}
		}
		if !found && m.store.Config.CompactModel != "" {
			s.errs = append(s.errs, "Saved compaction model is unavailable; choose explicitly.")
		}
		return nil
	})
}

func (s *compactSheet) width(*Model) int { return 76 }

func (s *compactSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Compact", "choose who writes the summary", w), ""}
	from, to := window(len(s.opts), s.cur, max(1, (h-9)/2))
	for i := from; i < to; i++ {
		o := s.opts[i]
		line := paint(cText, o.label)
		if o.model != "" {
			line = paint(cText, fit(o.label, 18)) + paint(cGreen, o.model)
		}
		if o.prune {
			line += dim(" · experimental: keeps steps word for word")
		}
		if isDefault(m, o) {
			line += dim(" · default")
		}
		out = append(out, sheetRow(line, i == s.cur, w))
	}
	if s.loading {
		out = append(out, dim("  finding the models installed…"))
	}
	var details []string
	for _, e := range s.errs {
		for j, line := range wrap(e, max(8, w-4)) {
			prefix := "    "
			if j == 0 {
				prefix = "  " + paint(cYellow, "! ")
			}
			details = append(details, prefix+dim(line))
		}
	}
	footer := []string{""}
	for _, note := range []string{"External summaries start fresh; /rewind keeps the original.", "Native automatic compaction is unchanged."} {
		for _, line := range wrap(note, max(8, w-2)) {
			footer = append(footer, dim("  "+line))
		}
	}
	footer = append(footer, "", keysFit(w, "↑↓", "choose", "enter", "compact", "d", "make default", "esc", "cancel"))
	room := max(0, h-len(out)-len(footer))
	if len(details) > room {
		room = max(0, room-1)
		s.details = min(max(0, s.details), max(0, len(details)-room))
		out = append(out, details[s.details:min(len(details), s.details+room)]...)
		out = append(out, dim(fmt.Sprintf("  details %d–%d/%d · pgup/pgdn", min(len(details), s.details+1), min(len(details), s.details+room), len(details))))
	} else {
		s.details = 0
		out = append(out, details...)
	}
	out = append(out, footer...)
	if len(out) > h {
		out = out[:max(0, h)]
	}
	return out
}

func (s *compactSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "pgup":
		s.details = max(0, s.details-6)
	case "pgdown":
		s.details += 6
	case "up", "k", "shift+tab":
		s.cur = max(0, s.cur-1)
	case "down", "j", "tab":
		s.cur = min(len(s.opts)-1, s.cur+1)
	case "d":
		if len(s.opts) == 0 || s.cur < 0 || s.cur >= len(s.opts) {
			return nil
		}
		o := s.opts[s.cur]
		m.store.Config.CompactKind, m.store.Config.CompactModel = string(o.kind), o.model
		if o.prune {
			m.store.Config.CompactKind, m.store.Config.PruneModel = prunerKind, o.model
		}
		if err := m.store.SaveConfig(); err != nil {
			m.flash(err.Error(), true)
		} else {
			m.flash("Compaction default saved; native automatic compaction is unchanged", false)
		}
	case "enter":
		if len(s.opts) == 0 || s.cur < 0 || s.cur >= len(s.opts) {
			return nil
		}
		m.sheet = nil
		c := m.host
		if c == nil || c.key != s.sess || c.client == nil {
			return nil
		}
		o := s.opts[s.cur]
		if o.model == "" {
			m.flash("compacting "+s.name+"…", false)
			cl := c.client
			return hostCmd(func() error { return cl.Send("/compact") })
		}
		if st := c.sess.Info.State; st == "working" || st == "blocked" || st == "starting" {
			m.flash("it's working: let the turn end, then #compact", true)
			return nil
		}
		return m.compactBy(c, s.id, o, "")
	}
	return nil
}

// prunerKind is CompactKind for a default that prunes with PruneModel.
const prunerKind = "ollama-prune"

func pruner(model string) summarizer {
	return summarizer{kind: ollama.Kind, label: "Ollama prune", model: model, prune: true}
}

func isDefault(m *Model, o summarizer) bool {
	if o.prune {
		return m.store.Config.CompactKind == prunerKind && o.model == m.store.Config.PruneModel
	}
	return string(o.kind) == m.store.Config.CompactKind && o.model == m.store.Config.CompactModel
}

// pruneKeep is the probability at or over which a step is kept: tev1
// scores what the work still needs around 0.4–0.55, the unrelated under
// 0.3, so 0.5 would drop a failing test's output.
// ponytail: tuned on one sample; measure on our own sessions and make it
// a setting if it drops what's still needed or keeps too much.
const pruneKeep = 0.3

// compactBy has o summarise session c and carries it on from the summary,
// with draft in its box.
func (m *Model) compactBy(c *hostConn, id string, o summarizer, draft string) tea.Cmd {
	if c.sess.Info.Proto < 9 {
		m.flash("Restart this session’s host before local compaction", true)
		return nil
	}
	expected := c.sess.Info
	left, key := leftOf(c), c.key
	by := o.label + " " + o.model
	// The dock's working line shows it, as it does Claude Code's own.
	sess := c.sess
	sess.MarkCompacting(time.Now())
	// carry starts the fresh conversation on prompt; the one it had is
	// left for /rewind.
	carry := func(by, prompt string) tea.Msg {
		newID, _ := host.NewSessionID()
		err := hangUp(id, func(cl *host.Client) error {
			return cl.CompactedIfUnchanged(newID, prompt, left, expected)
		})
		if err != nil {
			return doneMsg{err: err}
		}
		return rewoundMsg{key: key, draft: draft, text: "compacted by " + by + " · the conversation it had is kept (/rewind)"}
	}
	done := func(m *Model, msg tea.Msg) tea.Cmd {
		sess.MarkCompacting(time.Time{})
		return func() tea.Msg { return msg }
	}
	if o.prune {
		goal, steps := sess.PruneSteps()
		if len(steps) == 0 {
			sess.MarkCompacting(time.Time{})
			m.flash("nothing to prune yet: only the latest turns have tool steps", true)
			return nil
		}
		return later(func() tea.Msg {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
			defer cancel()
			scores, err := ollama.Keep(ctx, o.model, goal, steps)
			if err != nil {
				return doneMsg{err: fmt.Errorf("%s couldn't judge it: %w", by, err)}
			}
			return pruneScores(scores)
		}, func(m *Model, msg tea.Msg) tea.Cmd {
			scores, ok := msg.(pruneScores)
			if !ok {
				return done(m, msg)
			}
			keep, kept := make([]bool, len(scores)), 0
			for i, p := range scores {
				if keep[i] = p >= pruneKeep; keep[i] {
					kept++
				}
			}
			if kept == len(keep) {
				return done(m, doneMsg{err: fmt.Errorf("%s judged all %d steps still needed; conversation left unchanged", by, kept)})
			}
			by := fmt.Sprintf("%s (kept %d of %d steps)", by, kept, len(keep))
			// Built here, on the UI, where the session is; a turn since
			// fails CompactedIfUnchanged and leaves it as it was.
			prompt := convo.PrunedPrompt(by, sess.Pruned(keep))
			return later(func() tea.Msg { return carry(by, prompt) }, done)
		})
	}
	text := sess.PlainText()
	return later(func() tea.Msg {
		sz, ok := agent.As[agent.Summarizer](o.kind)
		if !ok {
			return doneMsg{err: fmt.Errorf("%s can't summarise", o.label)}
		}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
		defer cancel()
		summary, err := sz.Summarize(ctx, o.model, convo.CompactSystem, text)
		if err != nil {
			return doneMsg{err: fmt.Errorf("%s couldn't summarise it: %w", by, err)}
		}
		if len(strings.Fields(text)) > 100 && len(strings.Fields(summary)) < 12 {
			return doneMsg{err: fmt.Errorf("%s returned too little detail to safely compact; conversation left unchanged", by)}
		}
		return carry(by, convo.CompactedPrompt(by, summary))
	}, done)
}

// pruneScores are a decision model's keep probabilities, one a step.
type pruneScores []float64

// cheapCompact is who summarises the session on c when rush compacts it
// rather than its harness: the #compact default, else the agent's quick
// model (Claude's Haiku). Not ok when it can't: no such model, a harness
// that can't carry on fresh, an old host, or a turn under way.
func (m *Model) cheapCompact(c *hostConn, a *fleet.Agent) (summarizer, bool) {
	kind := agent.Migrated(firstNonEmpty(c.sess.Info.Kind, a.Kind))
	o := summarizer{kind: agent.Kind(m.store.Config.CompactKind), label: agentName(m.store.Config.CompactKind), model: m.store.Config.CompactModel}
	if m.store.Config.CompactKind == prunerKind {
		o = pruner(m.store.Config.PruneModel)
	}
	if q, ok := agent.As[agent.Querier](kind); ok && o.model == "" {
		quick, _ := q.QueryModels()
		o = summarizer{kind: kind, label: agentName(string(kind)), model: quick}
	}
	st := c.sess.Info.State
	return o, o.model != "" && c.client != nil && c.sess.Info.Proto >= 9 && agent.Supports(kind, agent.FeatureRewind) &&
		st != "working" && st != "blocked" && st != "starting"
}

// compactTyped is a typed /compact. On a cold cache the cheaper model of
// cheapCompact writes the summary, rather than the session's own model
// reading it all again uncached; warm, the harness compacts it itself
// from its cache, which costs less. /compact native, or with
// instructions, is always the harness's own.
func (m *Model) compactTyped(c *hostConn, a *fleet.Agent, arg string) (tea.Cmd, bool) {
	if arg == "native" && c.client != nil {
		cl := c.client
		return hostCmd(func() error { return cl.Send("/compact") }), true
	}
	o, ok := m.cheapCompact(c, a)
	if _, cold := c.sess.CacheCold(time.Now()); arg != "" || !ok || !cold {
		return nil, false
	}
	return m.compactBy(c, a.ID, o, ""), true
}
