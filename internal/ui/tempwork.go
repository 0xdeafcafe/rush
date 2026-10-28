package ui

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// tempShown is the least temp work worth pointing out.
const tempShown = 1 << 20

// tempLoud is temp work big enough to point out in yellow.
const tempLoud = 1 << 30

// tempMsg brings temp-work sizes measured in the background.
type tempMsg map[string]fleet.TempSize

// measureTemp walks the temp folders of the agents whose temp work may have
// changed, in the background and one batch at a time: never-measured ones
// once, finished ones again only after they've done something, running ones
// every ten minutes (longer for ones slow to walk).
func (m *Model) measureTemp() tea.Cmd {
	if m.measuring || m.tick%5 != 1 {
		return nil
	}
	due := m.loader.Temp.Due(m.snap.Agents, time.Now())
	if len(due) == 0 {
		return nil
	}
	m.measuring = true
	return m.nextTemp(due)
}

// Publish each size as soon as it is ready. A huge cache must not hold
// every smaller session's measurement until the whole batch completes.
func (m *Model) nextTemp(due []*fleet.Agent) tea.Cmd {
	if len(due) == 0 {
		m.measuring = false
		return nil
	}
	a := *due[0]
	return func() tea.Msg {
		at := time.Now()
		n := fleet.DiskUsageBackground(a.TempDirs())
		return tempMeasuredMsg{key: a.Key, size: fleet.TempSize{Bytes: n, At: at, Took: time.Since(at)}, remaining: due[1:]}
	}
}

type tempMeasuredMsg struct {
	remaining []*fleet.Agent
	key       string
	size      fleet.TempSize
}

func (m *Model) onTempMeasured(msg tempMeasuredMsg) tea.Cmd {
	m.onTemp(tempMsg{msg.key: msg.size})
	// Finish the captured queue so frequently changing sessions cannot starve old ones.
	m.measuring = len(msg.remaining) > 0
	return m.nextTemp(msg.remaining)
}

func (m *Model) onTemp(msg tempMsg) {
	m.measuring = false
	m.loader.Temp.Set(msg)
	for _, a := range m.snap.Agents {
		if e, ok := msg[a.Key]; ok {
			a.Temp = e.Bytes
		}
	}
	m.rebuild()
}

// tempTotal is all the agents' temp work.
func (m *Model) tempTotal() int64 {
	var n int64
	for _, a := range m.snap.Agents {
		n += a.Temp
	}
	return n
}

// disk formats bytes of disk the way mem formats memory.
func disk(n int64) string {
	switch {
	case n >= 1<<30:
		return fmt.Sprintf("%.1fG", float64(n)/(1<<30))
	case n >= 1<<20:
		return fmt.Sprintf("%dM", n>>20)
	case n > 0:
		return fmt.Sprintf("%dK", n>>10)
	}
	return "–"
}

// askClean opens Delete on an agent's temp work.
func (m *Model) askClean(a *fleet.Agent) tea.Cmd {
	if a == nil {
		m.flash("select an agent first", true)
		return nil
	}
	if a.PID != 0 {
		m.flash(a.DisplayName+" is still running; its temp work may be in use · stop it first (ctrl+x)", true)
		return nil
	}
	if a.Temp < tempShown {
		m.flash(a.DisplayName+" has no temp work to clean", false)
		return nil
	}
	return m.openDelete(nil, doomed{agents: []*fleet.Agent{a}})
}

// askCleanAll opens Delete on the temp work of every agent that has
// finished; running agents are left alone.
func (m *Model) askCleanAll() tea.Cmd {
	var list []*fleet.Agent
	for _, a := range m.snap.Agents {
		if a.PID == 0 && a.Temp >= tempShown {
			list = append(list, a)
		}
	}
	if len(list) == 0 {
		m.flash("no finished agent has temp work to clean", false)
		return nil
	}
	return m.openDelete(nil, doomed{agents: list})
}

// cleanTemp deletes agents' temp work in the background, then measures it
// again.
func (m *Model) cleanTemp(list []*fleet.Agent) tea.Cmd {
	var total int64
	for _, a := range list {
		total += a.Temp
	}
	m.flash("cleaning "+disk(total)+" of temp work…", false)
	return func() tea.Msg {
		sizes := tempMsg{}
		var failed error
		for _, a := range list {
			if err := fleet.CleanTemp(a); err != nil && failed == nil {
				failed = err
			}
			sizes[a.Key] = fleet.TempSize{Bytes: fleet.DiskUsage(a.TempDirs()), At: time.Now()}
		}
		var freed int64
		for _, a := range list {
			freed += a.Temp - sizes[a.Key].Bytes
		}
		return cleanedMsg{sizes: sizes, freed: freed, err: failed}
	}
}

type cleanedMsg struct {
	sizes tempMsg
	freed int64
	err   error
}

func (m *Model) onCleaned(msg cleanedMsg) {
	m.loader.Temp.Set(msg.sizes)
	for _, a := range m.snap.Agents {
		if e, ok := msg.sizes[a.Key]; ok {
			a.Temp = e.Bytes
		}
	}
	m.rebuild()
	if msg.err != nil {
		m.fail("freed " + disk(msg.freed) + " · " + msg.err.Error())
		return
	}
	m.flash("freed "+disk(msg.freed)+" of temp work", false)
}

// markDone puts an agent away in Earlier, or back. Put away, it shouldn't hold memory,
// so an idle process of its goes too (a message resumes it); only one still
// working asks first. Temp work stays; /clean is for that.
func (m *Model) markDone(a *fleet.Agent) tea.Cmd {
	if a == nil {
		m.flash("select an agent first", true)
		return nil
	}
	m.didStep("done")
	if a.Done {
		m.toggleDone(a)
		return nil
	}
	done := func() tea.Cmd {
		m.toggleDone(a)
		switch {
		case a.Interactive:
			return nil // it's open in a terminal: leave it be
		case a.Rush:
			// Its idle host may be gone and the agents it spawned still at work.
			m.flash("done: "+a.DisplayName+" · it stopped with all it started, a message resumes it", false)
			id := a.ID
			return cmdErr("", func() error { return host.StopAll(id) })
		case a.PID == 0:
			return nil // nothing resident
		}
		m.flash("done: "+a.DisplayName+" · its process stopped, a message resumes it", false)
		return cmdErr("", func() error { return stopOutside(a) })
	}
	if !a.Live() && !a.Busy() {
		return done()
	}
	c := &confirmation{question: "Mark " + a.DisplayName + " done?", onYes: done}
	if a.Interactive {
		c.detail = "it's still working, in its terminal; it keeps going there"
	} else {
		c.detail = "it's still working · y stops it"
	}
	m.confirm = c
	return nil
}

// dismiss is ctrl+x: the agent goes for good. It stops with all it set
// going, its shells and the agents it spawned, and leaves the list; its
// transcript stays on disk. One mid-turn asks first, ctrl+x again saying
// yes; one in a terminal asks too, since that ends it there.
func (m *Model) dismiss(a *fleet.Agent) tea.Cmd {
	if a == nil {
		return nil
	}
	if isRoomRow(a) || a.Past || a.Interactive {
		return m.askClose(a)
	}
	if a.Busy() {
		m.confirm = &confirmation{question: "Stop " + a.DisplayName + " and hide it?",
			detail:  "it's mid-turn · it stops with all it started · transcript kept on disk",
			yesText: "stop", again: m.boundKey("list.stop"), onYes: func() tea.Cmd { return m.closeAgent(a) }}
		return nil
	}
	return m.closeAgent(a)
}

// askClose is the one way to close an agent, esc twice in its Session or
// ctrl+] x: y hides it (stopped, gone from the list for good; its
// conversation kept on disk), z stops it and keeps it in the list, r
// restarts it, s switches its harness or model, x deletes it for good.
func (m *Model) askClose(a *fleet.Agent) tea.Cmd {
	if a == nil {
		return nil
	}
	if isRoomRow(a) {
		return m.askHideRoom(a)
	}
	c := &confirmation{question: "Hide " + a.DisplayName + "?", yesText: "hide", onYes: func() tea.Cmd { return m.closeAgent(a) }}
	switch {
	case a.Past:
		c.question, c.onYes = a.DisplayName+" is a past conversation", nil
	case a.Interactive:
		c.detail = "hides it from the list · it runs in its terminal: this ends that · transcript kept"
	default:
		c.detail = "y hides it from the list and stops it · z just stops it and keeps it in the list · transcript kept on disk"
	}
	// A stop, not a hide: the run ends, the agent stays in the list, its
	// transcript stays. It comes before restart, so the gentlest reads first.
	if !a.Past && (a.PID != 0 || (a.Live() && a.Worker != nil)) {
		c.more = append(c.more, confirmChoice{"z", "stop, keep it", func() tea.Cmd { return m.stopAgent(a) }})
	}
	if !a.Past && !a.Interactive {
		c.more = append(c.more, confirmChoice{"r", "restart", func() tea.Cmd { return m.restart(a, "") }})
	}
	if h := m.host; h != nil && h.key == a.Key && h.client != nil {
		c.more = append(c.more, confirmChoice{"s", "switch harness or model", func() tea.Cmd { return m.openSwitchSheet(h) }})
	}
	if _, ok := agent.As[agent.Remover](agent.Kind(a.Kind)); ok && !a.Interactive {
		c.more = append(c.more, confirmChoice{"x", "delete for good", func() tea.Cmd {
			return cmdErr("deleted "+a.DisplayName, func() error { return removeOutside(a) })
		}})
	}
	if c.onYes == nil && len(c.more) == 0 {
		m.flash(a.DisplayName+" is a past conversation: nothing runs to close", false)
		return nil
	}
	m.confirm = c
	return nil
}

// closeAgent hides a for good and stops it; one in a terminal is ended there.
// The hide is in the overlay, so no reading lists it again; its transcript stays.
func (m *Model) closeAgent(a *fleet.Agent) tea.Cmd {
	next := ""
	if m.sel == a.Key {
		next = m.neighbour(a.Key)
	}
	m.store.Hide(a.Key)
	_ = m.store.SaveOverlay()
	m.refresh()
	if next != "" {
		m.sel = next
	}
	m.flash("hidden: "+a.DisplayName, false)
	return m.stopRun(a, "")
}

// stopAgent ends a's run and leaves it in the list: a stop, not a hide. The
// run stops but the agent keeps its place and its transcript, so it comes
// back with a message; it is a pause, not a delete.
func (m *Model) stopAgent(a *fleet.Agent) tea.Cmd {
	if a.PID == 0 && !a.Live() {
		m.flash(a.DisplayName+" isn't running", false)
		return nil
	}
	m.toDone(a)
	m.flash("stopping "+a.DisplayName+"\u2026", false)
	return m.stopRun(a, "stopped "+a.DisplayName)
}

// toDone puts a away in Earlier when it's stopped or killed: a stopped agent has
// nothing left to show among the working ones. A message brings it back.
func (m *Model) toDone(a *fleet.Agent) {
	if _, done := m.store.Overlay.Done[a.Key]; !done && !a.Past {
		m.toggleDone(a)
	}
}

// undone brings a back from put away when you send it something: that holds it
// against all else, so a message is what brings it back.
func (m *Model) undone(a *fleet.Agent) {
	if _, done := m.store.Overlay.Done[a.Key]; done {
		delete(m.store.Overlay.Done, a.Key)
		a.Done = false
		_ = m.store.SaveOverlay()
		m.rebuild()
	}
}

// stopRun ends a's run as its own kind needs: a terminal session gets a
// SIGTERM, a rush session its own stop, an outside one its stop. msg is the
// flash on success, "" for none (a hide has said its own).
func (m *Model) stopRun(a *fleet.Agent, msg string) tea.Cmd {
	if a.Interactive {
		pid := a.PID
		return cmdErr(msg, func() error { return actions.Terminate(pid) })
	}
	switch {
	case a.Rush:
		// With the agents it spawned: none works on once it's stopped.
		id := a.ID
		return cmdErr(msg, func() error { return host.StopAll(id) })
	case a.PID == 0:
		return nil
	}
	return cmdErr(msg, func() error { return stopOutside(a) })
}
