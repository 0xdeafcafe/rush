package ui

import (
	"maps"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// A plugin with the sidebar capability arranges the agent list its own way:
// it offers a group-by mode, plugin:<name>, whose sections are the plugin's
// and whose rows carry the names it gives. Agents it doesn't place go to
// otherSection, folded.

const (
	pluginGroupPrefix = "plugin:"
	otherSection      = "Other"
)

// groupModes are the group-by modes on offer: rush's own, then one for
// each plugin that arranges the list.
func (m *Model) groupModes() []string {
	out := append([]string(nil), groupModes...)
	for _, s := range m.sidebars {
		out = append(out, pluginGroupPrefix+s.Plugin)
	}
	return out
}

// groupLabel is how a group-by mode is named to you.
func (m *Model) groupLabel(mode string) string {
	if name, ok := strings.CutPrefix(mode, pluginGroupPrefix); ok {
		for _, s := range m.sidebars {
			if s.Plugin == name {
				return s.Label()
			}
		}
	}
	return mode
}

// activeSidebar is the plugin arrangement the list is grouped by, if any.
// Embedded shows one session and is never arranged.
func (m *Model) activeSidebar() *plugin.Sidebar {
	name, ok := strings.CutPrefix(m.store.Config.GroupBy, pluginGroupPrefix)
	if !ok || m.hostedAlone() {
		return nil
	}
	for i := range m.sidebars {
		if m.sidebars[i].Plugin == name {
			return &m.sidebars[i]
		}
	}
	return nil
}

// loadSidebars reads the plugins' arrangements again where their files
// changed.
func (m *Model) loadSidebars() {
	if m.hostedAlone() {
		m.sidebars = nil
		return
	}
	m.sidebars = m.sidebarFiles.Load()
}

// nameAgents gives the agents the arrangement places its names, except
// where you renamed one in rush, and gives back the names it took over
// from an arrangement no longer in use.
func (m *Model) nameAgents(sb *plugin.Sidebar) {
	defer m.namePeers()
	for a, name := range m.renamed {
		a.DisplayName = name
	}
	clear(m.renamed)
	if sb == nil {
		return
	}
	if m.renamed == nil {
		m.renamed = map[*fleet.Agent]string{}
	}
	for _, a := range m.snap.Agents {
		p, ok := sb.Agents[a.SessionID]
		if !ok || p.Name == "" || a.SessionID == "" || m.store.Overlay.Names[a.Key] != "" {
			continue
		}
		m.renamed[a] = a.DisplayName
		a.DisplayName = p.Name
	}
}

// namePeers tells the conversation view which session each codename is,
// so a message sent to another session names it as the list does. A
// codename used again goes to the session that has it now.
func (m *Model) namePeers() {
	var codes map[string]string
	for _, ad := range agent.All() {
		cn, ok := ad.(agent.Codenamer)
		switch {
		case !ok:
		case codes == nil:
			codes = cn.Codenames() // read only: one provider's needs no copy
		default:
			codes = maps.Clone(codes)
			maps.Copy(codes, cn.Codenames())
		}
	}
	names := make(map[string]string, len(codes))
	for _, a := range m.snap.Agents {
		c := codes[a.SessionID]
		if _, ok := names[c]; c == "" || ok && a.PID == 0 {
			continue
		}
		names[c] = a.DisplayName
	}
	convo.SetPeers(names)
}

// sidebarPlace is the section an agent goes in under the arrangement, how
// high that section ranks, and the agent's place in it.
func sidebarPlace(sb *plugin.Sidebar, a *fleet.Agent) (section string, rank, order int) {
	if p, ok := sb.Agents[a.SessionID]; ok && a.SessionID != "" {
		for i, s := range sb.Sections {
			if s.Title == p.Section {
				return s.Title, i, p.Order
			}
		}
	}
	return otherSection, len(sb.Sections), 0
}
