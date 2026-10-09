package ui

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// permissionChoices prefers what the running harness actually advertises.
// New sessions can only offer modes the adapter declares at startup.
func permissionChoices(kind agent.Kind, c *hostConn) []agent.Choice {
	if c != nil && c.sess != nil && sessionAgent(c) == kind && len(c.sess.Info.PermissionModes) > 0 {
		var out []agent.Choice
		for _, v := range c.sess.Info.PermissionModes {
			note := strings.TrimSpace(v.Name)
			if v.Description != "" {
				if note != "" {
					note += " · "
				}
				note += v.Description
			}
			out = append(out, agent.Choice{ID: v.ID, Note: note})
		}
		return out
	}
	ch, _ := agent.ChoicesOf(kind)
	return ch.Modes
}

func permissionKnown(kind agent.Kind, c *hostConn, mode string) bool {
	for _, v := range permissionChoices(kind, c) {
		if v.ID == mode {
			return true
		}
	}
	return false
}

// bypassMode is the harness's don't-ask-again mode, when it advertises
// one: #yolo's, and the approval card's o.
func bypassMode(kind agent.Kind, c *hostConn) string {
	for _, v := range permissionChoices(kind, c) {
		switch v.ID {
		case "bypassPermissions", "full-access", "yolo", "auto-approve":
			return v.ID
		}
	}
	return ""
}

func (m *Model) setPermission(c *hostConn, mode string) tea.Cmd {
	if c == nil || c.client == nil && !c.sleeping {
		m.flash("Permissions can be changed in a hosted session; use #rush first", true)
		return nil
	}
	if !permissionKnown(sessionAgent(c), c, mode) {
		m.flash("This harness does not offer permission mode "+mode, true)
		return nil
	}
	if c.client == nil {
		return m.wakeHostThen(c, func(m *Model, next *hostConn) tea.Cmd { return m.setPermission(next, mode) })
	}
	cl := c.client
	return sheetDo(func() (struct{}, error) { return struct{}{}, cl.SetPermissionMode(mode) }, func(m *Model, _ struct{}, err error) tea.Cmd {
		if m.host != c || c.client != cl {
			return nil
		}
		if err != nil {
			m.flash("Permissions unchanged: "+err.Error(), true)
		} else {
			m.flash("Permission change requested: "+mode+" · applies to subsequent tools or turns", false)
		}
		return nil
	})
}

// permissionCommand always opens the same row as Shift+Tab. An explicit mode
// or #yolo is applied only if that harness advertises that exact capability.
func (m *Model) permissionCommand(c *hostConn, arg string, yolo bool) tea.Cmd {
	o := m.nextStart(m.startDir())
	if c != nil {
		o = m.sessionStart(c)
	}
	if yolo {
		if arg != "" {
			m.flash("#yolo takes no arguments; use #perm to choose a permission mode", true)
			return nil
		}
		if arg = bypassMode(agent.Kind(o.kind), c); arg == "" {
			m.flash("This harness does not advertise a bypass permission mode", true)
			return nil
		}
	}
	if arg != "" {
		if !permissionKnown(agent.Kind(o.kind), c, arg) {
			m.flash("Unknown permission mode "+arg+" · #perm shows supported modes", true)
			return nil
		}
		if c != nil {
			return m.setPermission(c, arg)
		}
		o.mode, o.modeSet = arg, true
		m.startOver = &o
		m.flash("Next session permissions: "+arg, false)
		return nil
	}
	cmd := m.openSetupSheet(c, o)
	if s, ok := m.sheet.(*startSheet); ok {
		s.row, s.applyOnly = 6, true
	}
	return cmd
}
