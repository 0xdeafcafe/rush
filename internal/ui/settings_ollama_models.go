package ui

import (
	"context"
	"fmt"
	"maps"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/adapters/ollama"
	"github.com/0xdeafcafe/rush/internal/agent"
)

func (m *Model) ollamaModelSettings(model string) []setting {
	value := "automatic"
	if n := ollama.ContextSize(model); n > 0 {
		value = tokens(int64(n))
	}
	info := "Loaded on demand when a session starts. Context changes apply to new sessions."
	if known, ok := ollama.CachedModel(model); ok {
		if known.Context > 0 {
			info += fmt.Sprintf(" Currently loaded: %s tokens.", tokens(int64(known.Context)))
		}
		if known.MaxContext > 0 {
			info += fmt.Sprintf(" Model maximum: %s tokens.", tokens(int64(known.MaxContext)))
		}
	}
	contextRow := setting{label: "Context size", value: value, what: info, keys: []string{"enter", "change"}}
	contextRow.key = func(s string) (tea.Cmd, bool) {
		if s != "enter" {
			return nil, false
		}
		m.ask("context tokens (0 = automatic)", strconv.Itoa(ollama.ContextSize(model)), func(input string) tea.Cmd {
			n, err := strconv.Atoi(strings.TrimSpace(input))
			if err != nil {
				m.flash("Enter a whole number of context tokens", true)
				return nil
			}
			return sheetDo(func() (struct{}, error) { return struct{}{}, ollama.SetContextSize(model, n) }, func(m *Model, _ struct{}, err error) tea.Cmd {
				if err != nil {
					m.flash(err.Error(), true)
				} else {
					m.flash("Context saved for new Ollama sessions", false)
				}
				return nil
			})
		})
		return nil, true
	}
	refresh := setting{label: "Refresh local models", what: "Starts Ollama if needed and discovers installed models. Models load into memory only when used.", keys: []string{"enter", "refresh"}}
	refresh.key = func(s string) (tea.Cmd, bool) {
		if s != "enter" {
			return nil, false
		}
		return sheetDo(func() ([]ollama.Model, error) {
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()
			return ollama.InstalledModels(ctx)
		}, func(m *Model, models []ollama.Model, err error) tea.Cmd {
			if err != nil {
				m.flash(err.Error(), true)
				return nil
			}
			choices := make([]agent.Choice, 0, len(models))
			for _, model := range models {
				if model.CanCode() {
					choices = append(choices, agent.Choice{ID: model.Name, Context: int64(model.MaxContext), Note: model.Params + " · loads on demand"})
				}
			}
			if m.listed == nil {
				m.listed = map[string][]agent.Choice{}
			}
			for _, k := range []agent.Kind{ollama.Kind, ollama.CodexKind, ollama.PiKind, ollama.VibeKind} {
				m.listed[string(k)] = choices
			}
			m.flash(fmt.Sprintf("Ollama ready · %d installed models · %d support coding tools", len(models), len(choices)), false)
			return nil
		}), true
	}
	rows := []setting{refresh}
	if model != "" {
		rows = []setting{contextRow, refresh}
	}
	for _, model := range ollama.CachedInstalledModels() {
		if !model.CanCode() && !model.Can("decision") { // decision models are under Pruning
			value, why := "no coding tools", "Installed in Ollama. This model does not advertise tool calling, which Rush coding harnesses require."
			rows = append(rows, setting{label: model.Name, value: value, what: why})
		}
	}
	return rows
}

// pulls are the Ollama pulls under way: model → percent done.
var pulls sync.Map

// decisionSection manages the Ollama decision models #compact prunes
// with: pull, mend, remove, and which.
func (m *Model) decisionSection() section {
	cfg := &m.store.Config
	var rows []setting
	inUse := cfg.PruneModel
	if inUse == "" {
		inUse = ollama.CachedDecisionModel()
	}
	for _, md := range ollama.CachedInstalledModels() {
		if !md.Can("decision") {
			continue
		}
		name := md.Name
		value := firstNonEmpty(md.Params, "installed")
		if name == inUse {
			value += " · prunes"
		}
		row := setting{label: name, value: value, keys: []string{"enter", "prune with it", "x", "remove"},
			what: "A decision model: it scores which tool steps a conversation still needs, and pruning keeps those word for word."}
		row.key = func(s string) (tea.Cmd, bool) {
			switch s {
			case "enter":
				cfg.PruneModel = name
				_ = m.store.SaveConfig()
				m.rebuild()
				m.flash("Pruning with "+name, false)
				return nil, true
			case "x":
				m.confirmThen("Remove "+name+" from Ollama?", func() tea.Cmd {
					return sheetDo(func() (struct{}, error) {
						ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
						defer cancel()
						return struct{}{}, ollama.Remove(ctx, name)
					}, func(m *Model, _ struct{}, err error) tea.Cmd {
						if err != nil {
							m.flash(err.Error(), true)
							return nil
						}
						if m.store.Config.PruneModel == name {
							m.store.Config.PruneModel = ""
							_ = m.store.SaveConfig()
						}
						m.rebuild()
						m.flash("Removed "+name, false)
						return nil
					})
				})
				return nil, true
			}
			return nil, false
		}
		rows = append(rows, row)
	}
	broken := ollama.Broken()
	for _, name := range slices.Sorted(maps.Keys(broken)) {
		why := broken[name]
		row := setting{label: name, value: "files missing", keys: []string{"enter", "pull again", "x", "remove"},
			what: "Ollama lists it but can't read it (" + why + "). Pulling it again fetches what's missing."}
		row.key = func(s string) (tea.Cmd, bool) {
			switch s {
			case "enter":
				return m.pullModel(name), true
			case "x":
				m.confirmThen("Remove "+name+" from Ollama?", func() tea.Cmd {
					return sheetDo(func() (struct{}, error) {
						ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
						defer cancel()
						return struct{}{}, ollama.Remove(ctx, name)
					}, func(m *Model, _ struct{}, err error) tea.Cmd {
						if err != nil {
							m.flash(err.Error(), true)
						}
						m.rebuild()
						return nil
					})
				})
				return nil, true
			}
			return nil, false
		}
		rows = append(rows, row)
	}
	pull := setting{label: "Pull a decision model", keys: []string{"enter", "pull"},
		what: "Downloads a decision model into Ollama by name: tev1 or nimble for text."}
	pull.key = func(s string) (tea.Cmd, bool) {
		if s != "enter" {
			return nil, false
		}
		m.ask("decision model to pull", "tev1", func(name string) tea.Cmd {
			if name = strings.TrimSpace(name); name == "" {
				return nil
			}
			return m.pullModel(name)
		})
		return nil, true
	}
	rows = append(rows, pull)
	// Judged by Codex on real sessions, tev1 ranked steps below Claude
	// Haiku (AUC 0.73 vs 0.84) at 1.6s a step: #compact only, for now.
	return section{title: "Pruning", note: "experimental · local decision models for #compact", rows: rows}
}

// pullModel pulls name, its progress flashed each second until it's in.
func (m *Model) pullModel(name string) tea.Cmd {
	if _, busy := pulls.LoadOrStore(name, 0); busy {
		m.flash(name+" is already being pulled", true)
		return nil
	}
	m.flash("pulling "+name+"…", false)
	var tick func() tea.Cmd
	tick = func() tea.Cmd {
		return tea.Tick(time.Second, func(time.Time) tea.Msg {
			return sheetMsg{apply: func(m *Model) tea.Cmd {
				pct, busy := pulls.Load(name)
				if !busy {
					return nil
				}
				m.flash(fmt.Sprintf("pulling %s · %d%%", name, pct), false)
				return tick()
			}}
		})
	}
	pull := sheetDo(func() (struct{}, error) {
		defer pulls.Delete(name)
		err := ollama.Pull(context.Background(), name, func(done, total int64) {
			pulls.Store(name, int(done*100/total))
		})
		if err != nil {
			return struct{}{}, err
		}
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		_, err = ollama.InstalledModels(ctx)
		return struct{}{}, err
	}, func(m *Model, _ struct{}, err error) tea.Cmd {
		if err != nil {
			m.flash(name+": "+err.Error(), true)
			return nil
		}
		m.rebuild()
		m.flash(name+" is ready", false)
		return nil
	})
	return tea.Batch(pull, tick())
}
