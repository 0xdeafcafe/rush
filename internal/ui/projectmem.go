package ui

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/memory"
)

// memSummary is a project's memory as Projects shows it: its notes by
// kind, and how many are worth a look.
type memSummary struct {
	p      agent.Profile
	root   string
	notes  int
	kinds  map[string]int
	review int
	at     time.Time
}

// memSummaries are Projects' reads of each project's memory, by root.
var memSummaries = offReads[string, memSummary]{}

// memSummaryOf is the project at root's memory, read again at most every
// half minute, or at once after a review (m.memCounts dropped).
func (m *Model) memSummaryOf(p *project) *memSummary {
	if m.memCounts == nil {
		m.memCounts = map[string]*memSummary{}
	}
	if s, ok := memSummaries.take(p.key); ok {
		m.memCounts[p.key] = &s
	}
	s := m.memCounts[p.key]
	if s != nil && time.Since(s.at) < 30*time.Second {
		return s
	}
	prof := m.store.Config.ActiveAccount().Profile()
	for _, a := range p.agents {
		if a.Acct.Dir != "" && agent.Supports(a.Acct.Kind, agent.FeatureMemory) {
			prof = a.Acct
			break
		}
	}
	mr, ok := agent.As[agent.MemoryReader](prof.Kind)
	if !ok {
		return s
	}
	root := p.key
	memSummaries.start(root, func() memSummary {
		mem := mr.Memory(prof, root, "")
		out := memSummary{p: prof, root: root, kinds: map[string]int{}, at: time.Now()}
		for _, f := range mem.Files {
			if f.Group != "Auto memory" || f.Name == "MEMORY.md" || f.Missing {
				continue
			}
			out.notes++
			out.kinds[firstNonEmpty(f.Kind, "untyped")]++
			if memory.Why(f, out.at) != "" {
				out.review++
			}
		}
		return out
	})
	return s
}

// projectMemory is a project's Memory section: how many notes of each
// kind, and how many to review, on a row enter reviews.
func (m *Model) projectMemory(p *project, w int, rows *[]workRow) {
	if !filepath.IsAbs(p.key) || !p.repo {
		return
	}
	s := m.memSummaryOf(p)
	if s == nil || s.notes == 0 {
		return
	}
	*rows = append(*rows, workRow{line: "", owner: p.key})
	meta := fmt.Sprintf("%d notes", s.notes)
	if s.notes == 1 {
		meta = "1 note"
	}
	*rows = append(*rows, workRow{line: psection("Memory", meta, w), owner: p.key})
	var kinds []string
	for _, k := range []string{"user", "feedback", "project", "reference", "untyped"} {
		if n := s.kinds[k]; n > 0 {
			kinds = append(kinds, paint(memTypeColour(k), fmt.Sprintf("%d %s", n, k)))
		}
	}
	line := " " + strings.Join(kinds, dim(" · "))
	if s.review > 0 {
		line += "   " + paint(cYellow, fmt.Sprintf("! %d to review", s.review)) + dim(" · enter")
	} else {
		line += "   " + dim("✓ nothing to review")
	}
	*rows = append(*rows, workRow{id: "m" + p.key, mem: s, owner: p.key, line: line})
}
