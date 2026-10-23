package ui

import (
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// An installed agent's part of its harness's page: whatever sections the
// agent adds itself. What its sessions start with is Every harness's and
// its own changes (settings_harnesses.go).

// agentExtras are the sections an agent adds to its page, by kind.
var agentExtras = map[agent.Kind]func(m *Model) []section{}

// agentSections are agent k's own sections, the advanced ones folded
// under a line of their own.
func (m *Model) agentSections(k agent.Kind) []section {
	var secs []section
	var advanced, extras []section
	if extra := agentExtras[k]; extra != nil {
		extras = extra(m)
	}
	extras = append(extras, m.fileSections(k)...)
	for _, s := range extras {
		if s.advanced {
			advanced = append(advanced, s)
		} else {
			secs = append(secs, s)
		}
	}
	if len(advanced) == 0 {
		return secs
	}
	var names []string
	for _, s := range advanced {
		names = append(names, s.title)
	}
	open := m.dialog.advanced
	toggle := setting{
		label: "Advanced",
		line: func(int) string {
			mark := "▸ "
			if open {
				mark = "▾ "
			}
			return dim(mark+"Advanced") + faint(" · "+strings.Join(names, ", "))
		},
		key: func(s string) (tea.Cmd, bool) {
			if s == "enter" || s == "right" || s == "left" || s == "space" {
				m.dialog.advanced = !m.dialog.advanced
				return nil, true
			}
			return nil, false
		},
		keys: []string{"enter", "show or hide"},
		about: func() (string, string, string) {
			return "Advanced", "What " + harnessName(string(k)) + " itself reads, beyond what rush starts it with: " + strings.Join(names, ", ") + ". Most people never need these.", ""
		},
	}
	secs = append(secs, section{rows: []setting{toggle}})
	if open {
		secs = append(secs, advanced...)
	}
	return secs
}

// modelTakes is what model takes in agent k: images and PDFs as its
// MediaReader says, and its context window, each only when known. Nothing
// here waits: Ollama's Reads asks in the background.
func modelTakes(k agent.Kind, model string) string {
	var out []string
	if reader, ok := agent.As[agent.MediaReader](k); ok {
		if has, ok := reader.Reads(model); ok {
			for _, m := range []struct {
				bit  agent.Media
				name string
			}{{agent.MediaImage, "images"}, {agent.MediaPDF, "PDFs"}} {
				if has&m.bit != 0 {
					out = append(out, paint(cGreen, "✓ ")+dim(m.name))
				} else {
					out = append(out, faint("– "+m.name))
				}
			}
		}
	}
	if windower, ok := agent.As[agent.ContextWindower](k); ok {
		if n := windower.ContextWindow(model); n > 0 {
			out = append(out, dim(tokens(n)+" context"))
		}
	}
	if len(out) == 0 {
		return faint("what it takes is unknown to rush until it's used")
	}
	return strings.Join(out, faint(" · "))
}

// agentModels are the models agent k's adapter offers, then any your
// sessions have run that it didn't list.
func (m *Model) agentModels(k agent.Kind) []agent.Choice {
	models := m.models(string(k))
	seen := map[string]bool{}
	for _, c := range models {
		seen[c.ID] = true
	}
	for _, a := range m.snap.Agents {
		if model := a.Spend.Model; model != "" && !seen[model] && a.Kind == string(k) && !strings.HasPrefix(model, "<") {
			seen[model] = true
			models = append(models, agent.Choice{ID: model, Note: "one your " + agentName(string(k)) + " sessions have run."})
		}
	}
	return models
}
