package ui

import (
	"context"
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/memory"
)

// Remembering: #remember <text>, or alt+k on a picked message, has a cheap
// model write a memory note from it (its name, description and type), then
// asks before it's kept: enter keeps it, e keeps it and opens it in the
// memory view's editor.

// rememberDraft is a cheap model's note; a variable so tests don't spend a
// call.
var rememberDraft = func(what, around string) (agent.MemoryNote, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	return memory.Draft(ctx, what, around)
}

// remember drafts a note from what, and the conversation around it, for
// the session in c, then asks to keep it.
func (m *Model) remember(c *hostConn, what, around string) tea.Cmd {
	if c == nil {
		m.flash("open a session to remember something for its project", true)
		return nil
	}
	k := sessionAgent(c)
	mk, ok := agent.As[agent.MemoryKeeper](k)
	if !ok || !agent.Supports(k, agent.FeatureMemory) {
		m.flash(harnessName(string(k))+" keeps no memory rush can write", true)
		return nil
	}
	if strings.TrimSpace(what+around) == "" {
		if d := m.memDraft; d != nil && d.c == c {
			m.memDraft = nil
			d.ask(m)
			return nil
		}
		m.flash("#remember <what to remember>, or alt+k on a message", true)
		return nil
	}
	p, cwd := m.memProfile(c)
	m.flash("writing the note…", false)
	return sheetDo(func() (agent.MemoryNote, error) { return rememberDraft(what, around) },
		func(m *Model, n agent.MemoryNote, err error) tea.Cmd {
			if err != nil {
				m.fail("couldn't write the note: " + err.Error())
				return nil
			}
			n.Title = firstNonEmpty(n.Title, n.Name)
			keep := func(edit bool) tea.Cmd {
				type kept struct {
					path   string
					review int
				}
				return sheetDo(func() (kept, error) {
					path, err := mk.Remember(p, cwd, n)
					if err != nil {
						return kept{}, err
					}
					r, _ := readMemReview(p, cwd, "")
					others := 0
					for _, it := range r.items {
						if it.f.Path != path {
							others++
						}
					}
					return kept{path, others}, nil
				},
					func(m *Model, k kept, err error) tea.Cmd {
						if err != nil {
							m.fail(err.Error())
							return nil
						}
						path := k.path
						c.mem = nil
						m.dropMemCache()
						msg := "remembered: " + n.Title + " · in the memory view"
						if k.review > 0 {
							msg += fmt.Sprintf(" · %d other notes worth a look: r on one there reviews them", k.review)
						}
						m.flash(msg, false)
						if edit && m.showView(c, "memory") {
							c.sel, c.memEd, c.memEdit = "mem:"+path, openDoc(path), true
						}
						return nil
					})
			}
			d := &memDraft{c: c, ask: func(m *Model) {
				m.confirm = &confirmation{
					question: "Remember this? " + n.Title,
					detail:   n.Type + " · " + n.Description + "\n\n" + n.Body,
					yesText:  "remember",
					onYes:    func() tea.Cmd { return keep(false) },
					more:     []confirmChoice{{key: "e", text: "remember and edit", do: func() tea.Cmd { return keep(true) }}},
				}
			}}
			// The draft takes a while: if you've moved on to something
			// else meanwhile, it waits rather than take your next key.
			if m.confirm != nil || m.sheet != nil || len(c.input) > 0 {
				m.memDraft = d
				m.flash("the note is ready · #remember alone shows it", false)
				return nil
			}
			d.ask(m)
			return nil
		})
}

// memDraft is a note drafted and waiting to be asked about.
type memDraft struct {
	c   *hostConn
	ask func(m *Model)
}

// rememberPicked remembers the picked message: its exchange, for a cheap
// model to pick the one thing worth keeping from.
func (m *Model) rememberPicked(c *hostConn) (tea.Cmd, bool) {
	text, ok := c.sess.ExchangeText(c.sel)
	if !ok {
		if t := selTurn(c); t != nil {
			text, ok = t.Answer(), t.Answer() != ""
		}
	}
	if !ok {
		return nil, false
	}
	return m.remember(c, "", text), true
}
