package ui

import (
	"context"
	"errors"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/remote"
)

// An attached session's changes view reads its own machine's git.
func init() { convo.RemoteGit = remote.Git }

// #on starts an agent on another machine's rush (a peer of rush remote add):
//
//	#on MACHINE [folder] [agent=KIND] task…
//
// It is one POST of what rush session --on MACHINE start sends. The folder
// is the machine's own (default its ~): it's read and checked there, never
// as a path here. The agent, its account, model and files stay on that
// machine; the Prompt's start options, presets and images are this one's and
// aren't carried. Nothing is retried: if no whole answer came, it says so and
// the session may exist there. submit empties the box before any command
// runs, so a failure puts the command back (onBack).

// onStart reads #on's argument against the peers kept in cfg.
func onStart(cfg remote.Config, arg string) (remote.Peer, remote.StartRequest, error) {
	var req remote.StartRequest
	name, rest, _ := strings.Cut(strings.TrimSpace(arg), " ")
	if name == "" {
		return remote.Peer{}, req, errors.New("#on MACHINE [folder] [agent=KIND] task")
	}
	p, ok := cfg.Peer(name)
	if !ok {
		return p, req, errors.New("no machine " + name + " · rush remote add it first")
	}
	req.Cwd = "~"
	rest = strings.TrimSpace(rest)
	if w, after, _ := strings.Cut(rest, " "); strings.HasPrefix(w, "/") || w == "~" || strings.HasPrefix(w, "~/") {
		req.Cwd, rest = w, strings.TrimSpace(after)
	}
	if w, after, _ := strings.Cut(rest, " "); strings.HasPrefix(w, "agent=") {
		req.Agent, rest = strings.TrimPrefix(w, "agent="), strings.TrimSpace(after)
	}
	if rest == "" {
		return p, req, errors.New("#on " + name + ": say what the agent should do")
	}
	req.Prompt = rest
	return p, req, nil
}

// onCommand runs #on with arg. submit has already emptied the box, so every
// way it fails puts the command back; nothing is retried, and while one start
// waits no other #on goes out.
func (m *Model) onCommand(arg string) tea.Cmd {
	draft := strings.TrimSpace("#on " + arg)
	if m.onBusy {
		m.onBack(draft)
		m.flash("a start on another machine is still waiting: wait for it before sending another", true)
		return nil
	}
	cfg, err := remote.LoadConfig()
	if err != nil {
		m.onBack(draft)
		m.flash(err.Error(), true)
		return nil
	}
	p, req, err := onStart(cfg, arg)
	if err != nil {
		m.onBack(draft)
		m.flash(err.Error(), true)
		return nil
	}
	if len(m.imgs.Path) > 0 {
		m.onBack(draft)
		m.flash("images aren't sent to another machine yet: remove them or start here", true)
		return nil
	}
	note := ""
	if m.startOver != nil {
		note = " · your start options aren't used there"
	}
	m.onBusy = true
	m.flash("starting on "+p.Name+"…", false)
	return func() tea.Msg {
		s, err := remote.StartOn(context.Background(), p, req)
		attached := err == nil && remote.Attached(p.Name)
		return applyMsg(func(m *Model) tea.Cmd {
			m.onBusy = false
			if err != nil {
				back := m.onBack(draft)
				switch {
				case errors.Is(err, remote.ErrMaybeStarted):
					m.flash(err.Error()+" · check the list before sending it again", true)
				case back:
					m.flash(err.Error()+" · your command is back in the box", true)
				default:
					m.flash(err.Error()+" · your command: "+draft, true)
				}
				return nil
			}
			text := "started " + s.Name + " on " + p.Name + note
			if !attached {
				text += " · rush remote attach " + p.Name + " shows it here"
			}
			m.flash(text, false)
			m.refresh()
			return nil
		})
	}
}

// onBack puts draft back in the box if nothing has been typed there since;
// it says whether it did. What was typed meanwhile is never overwritten.
func (m *Model) onBack(draft string) bool {
	if len(m.input) > 0 || m.inKind != inPrompt {
		return false
	}
	m.input = []rune(draft)
	m.setCursor(len(m.input))
	return true
}
