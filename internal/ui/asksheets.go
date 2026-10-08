package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// --- Claude Code's commands that need the session: /btw, /export, /subtask ---

// askClaude passes a control request to the agent's Claude Code through its
// host, and hands the answer to done when it comes (onHostLines).
func (m *Model) askClaude(c *hostConn, req map[string]any, done func(m *Model, r host.Reply) tea.Cmd) tea.Cmd {
	if c.client == nil {
		m.flash("that works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	if c.sess.Info.Proto < 2 {
		m.flash("this agent's host is older than that: it can once the host restarts (it does after resting idle)", true)
		return nil
	}
	c.askN++
	id := fmt.Sprintf("ui-%d", c.askN)
	if c.asks == nil {
		c.asks = map[string]func(*Model, host.Reply) tea.Cmd{}
	}
	c.asks[id] = done
	cl := c.client
	return hostCmd(func() error { return cl.Ask(id, req) })
}

// onReply hands a control request's answer to whoever asked.
func (m *Model) onReply(c *hostConn, r host.Reply) tea.Cmd {
	done := c.asks[r.ID]
	if done == nil {
		return nil // another rush's
	}
	delete(c.asks, r.ID)
	return done(m, r)
}

// sendAs sends text to the agent as a message of yours, as the box does.
func (m *Model) sendAs(c *hostConn, text string) tea.Cmd {
	if c.client == nil {
		m.flash("that works in rush-mode sessions · /rush moves this one over", true)
		return nil
	}
	m.markSending(c, text)
	cl := c.client
	return sendingVia(c.key, hostCmd(func() error { return cl.Send(text) }))
}

// relayPrompt asks Claude to pass msg on to its running subagent sa word
// for word: Claude Code takes no message into a subagent but its own
// SendMessage tool.
func relayPrompt(sa convo.Subagent, msg string) string {
	return "Pass this message on to your running subagent " + sa.ID + " (" + sa.Type + ") with SendMessage, word for word, then carry on with what you were doing. Don't act on it yourself.\n\nThe message: " + msg
}

// childRelayPrompt asks Codex to pass msg on to sa, one of its spawned
// threads: it takes input only from its parent's agent tools.
func childRelayPrompt(sa convo.Subagent, msg string) string {
	return "Pass this message on to your subagent " + oneLine(sa.Description) + " word for word: send_message while it's running, followup_task if it has finished. Then carry on with what you were doing. Don't act on it yourself.\n\nThe message: " + msg
}

// subtaskPrompt asks the agent to send a subagent off with the task, as
// Claude Code's /subtask does, and to carry on meanwhile.
func subtaskPrompt(task string) string {
	return "Send a subagent off in the background (the Agent tool with run_in_background, or spawn_agent) to do the task below. Give it everything it needs from our conversation so far, since it starts without it. Don't wait for it: carry on here, and when it's done tell me what it found.\n\nThe task: " + task
}

// --- /export ---

// exportSheet saves or copies the conversation as the plain text Claude
// Code's /export writes.
type exportSheet struct {
	conn     string
	text     string
	filename string
	dir      string
	cur      int // 0 copy, 1 save
	err      string
	loading  bool
	saving   bool // written in the background; enter waits for it
}

func (k *exportSheet) width(*Model) int { return 76 }

// openExport asks Claude Code for the conversation as text. Given a file
// name, it's saved there straight away.
func (m *Model) openExport(c *hostConn, dir, arg string) tea.Cmd {
	k := &exportSheet{conn: c.key, dir: dir, loading: true}
	cmd := m.askClaude(c, map[string]any{"subtype": "export_conversation"}, func(m *Model, r host.Reply) tea.Cmd {
		k.loading = false
		var v struct {
			Text     string `json:"text"`
			Filename string `json:"default_filename"`
		}
		if r.Error != "" || jsonx.Unmarshal(r.Body, &v) != nil {
			k.err = firstNonEmpty(r.Error, "Claude Code sent back something rush can't read")
			return nil
		}
		k.text, k.filename = v.Text, firstNonEmpty(v.Filename, "conversation.txt")
		if strings.TrimSpace(k.text) == "" {
			k.err = "there's nothing in the conversation yet"
		}
		if arg != "" && k.err == "" {
			return k.save(m, arg)
		}
		return nil
	})
	if cmd != nil && arg == "" {
		m.sheet = k
	}
	return cmd
}

func (k *exportSheet) save(m *Model, name string) tea.Cmd {
	// Relative to the agent's folder, not rush's.
	path := name
	if !filepath.IsAbs(name) && !strings.HasPrefix(name, "~") {
		path = filepath.Join(k.dir, name)
	}
	path = expand(path)
	if k.saving {
		return nil
	}
	k.saving = true
	text := k.text
	return later(func() error { return os.WriteFile(path, []byte(text), 0o644) }, func(m *Model, err error) tea.Cmd {
		k.saving = false
		if err != nil {
			m.flash("couldn't save it: "+err.Error(), true)
			return nil
		}
		if m.sheet == k {
			m.sheet = nil
		}
		m.flash("saved the conversation to "+tildify(path), false)
		return nil
	})
}

func (k *exportSheet) key(m *Model, _ tea.KeyPressMsg, s string) tea.Cmd {
	switch s {
	case "esc", "ctrl+c":
		m.sheet = nil
	case "up", "down", "tab", "shift+tab":
		k.cur = 1 - k.cur
	case "enter":
		if k.loading || k.err != "" {
			return nil
		}
		if k.cur == 1 {
			return k.save(m, k.filename)
		}
		m.copyText(k.text)
		m.sheet = nil
		m.flash(fmt.Sprintf("copied the conversation · %d lines", strings.Count(k.text, "\n")+1), false)
	}
	return nil
}

func (k *exportSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Export", "the conversation as plain text, as Claude Code writes it", w), ""}
	switch {
	case k.loading:
		return append(out, dim("  asking Claude Code for it…"), "", keysFit(w, "esc", "close"))
	case k.err != "":
		return append(out, paint(cRed, "  "+k.err), "", keysFit(w, "esc", "close"))
	}
	out = append(out, dim(fmt.Sprintf("  %d lines · %s", strings.Count(k.text, "\n")+1, approxTokens(int64(len(k.text))))), "")
	choices := []struct{ name, about string }{
		{"Copy to the clipboard", ""},
		{"Save to a file", tildify(filepath.Join(k.dir, k.filename)) + " · /export <name> saves elsewhere"},
	}
	for i, ch := range choices {
		out = append(out, sheetRow(paint(cText+bold, ch.name), i == k.cur, w))
		if ch.about != "" {
			out = append(out, "    "+faint(ansi.Truncate(ch.about, w-6, "…")))
		}
	}
	return append(out, "", keysFit(w, "↑↓", "choose", "enter", "go", "esc", "cancel"))
}
