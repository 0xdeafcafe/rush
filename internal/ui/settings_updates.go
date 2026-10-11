package ui

import (
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/harnessup"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/update"
)

// Settings, Updates: rush and each harness it runs, what's installed and
// what's newest, and enter updates the one under the cursor. Sessions
// run in hosts of their own that stay on the rush they started with; one
// on an older rush than the installed one restarts between turns, and
// the page can restart the idle ones now.

// updatesState is what the page and the top bar know, all of it read off
// the UI and landed by applyMsg.
type updatesState struct {
	rows      []harnessup.Status
	checked   bool
	checking  bool
	running   map[agent.Kind]bool // harnesses being updated
	rushNew   update.Info         // the newest rush, whether or not it's newer
	said      bool                // the count was flashed once
	stale     int                 // live sessions on an older rush than the installed one
	installed host.BinStamp       // the installed rush, as the last watch saw it
	sweeping  bool
	sweepList host.Lister // read by one sweep at a time
	checkedAt time.Time
}

// startStamp is the rush binary as it was when this one started.
var startStamp = host.ExeStamp()

// updatesCount is how many things have an update: the harnesses, and rush.
func (m *Model) updatesCount() int {
	n := 0
	for _, r := range m.upd.rows {
		if r.State() == harnessup.Available {
			n++
		}
	}
	if m.newer.Version != "" {
		n++
	}
	return n
}

// checkHarnesses looks for newer harnesses and rush, in the background;
// force asks the registries again rather than trusting what was kept.
func (m *Model) checkHarnesses(force bool) tea.Cmd {
	if m.offline || m.upd.checking {
		return nil
	}
	m.upd.checking = true
	return func() tea.Msg {
		ctx := context.Background()
		rows := harnessup.Check(ctx, force)
		latest, newer := update.Check(ctx)
		return applyMsg(func(m *Model) tea.Cmd {
			u := &m.upd
			u.rows, u.checked, u.checking, u.checkedAt = rows, true, false, time.Now()
			u.rushNew = latest
			if newer && m.newer.Version == "" {
				m.newer = latest
			}
			if n := m.updatesCount(); n > 0 && !u.said {
				u.said = true
				m.flash(fmt.Sprintf("%d updates · Settings › Updates", n), false)
			}
			return nil
		})
	}
}

// recheckAfter is checkHarnesses again once Every has passed.
func (m *Model) recheckAfter() tea.Cmd {
	if time.Since(m.upd.checkedAt) < harnessup.Every {
		return nil
	}
	return m.checkHarnesses(false)
}

// updateHarness runs the update of one harness, then looks again.
func (m *Model) updateHarness(r harnessup.Status) tea.Cmd {
	if m.upd.running == nil {
		m.upd.running = map[agent.Kind]bool{}
	}
	if m.upd.running[r.Kind] {
		m.flash("already updating "+r.Name+"…", false)
		return nil
	}
	m.upd.running[r.Kind] = true
	m.flash("updating "+r.Name+": "+r.Update.Label+"…", false)
	return func() tea.Msg {
		ctx := context.Background()
		err := harnessup.Run(ctx, r)
		agent.Recheck()
		rows := harnessup.Check(ctx, false)
		return applyMsg(func(m *Model) tea.Cmd {
			delete(m.upd.running, r.Kind)
			m.upd.rows = rows
			if err != nil {
				return func() tea.Msg { return doneMsg{err: err} }
			}
			return func() tea.Msg { return doneMsg{text: r.Name + " updated"} }
		})
	}
}

// watchHosts is the installed rush's binary looked at, and the sessions
// on an older one counted; with restart (Enter, you asking), the ones
// between turns are restarted on it. Never on its own: a new binary
// alone restarts nothing. It stats and lists files, so it runs off the UI.
func (m *Model) watchHosts(restart bool) tea.Cmd {
	if m.upd.sweeping {
		return nil
	}
	m.upd.sweeping = true
	return func() tea.Msg {
		var now host.BinStamp
		if exe, err := os.Executable(); err == nil {
			now = host.StampOf(exe)
		}
		var stale, restarted int
		if !now.Mod.IsZero() {
			stale, restarted = m.upd.sweepList.Sweep(now, restart)
		}
		return applyMsg(func(m *Model) tea.Cmd {
			m.upd.sweeping, m.upd.stale, m.upd.installed = false, stale, now
			if !now.Mod.IsZero() && now.Mod.After(startStamp.Mod) && !m.rebuilt {
				m.rebuilt = true
				if !m.rebuiltSaid {
					m.rebuiltSaid = true
					m.flash("rush was installed again · #reload runs it, sessions carry on", false)
				}
			}
			if restart {
				m.flash(fmt.Sprintf("%d sessions restarted on the installed rush · %d still busy", restarted, stale), false)
			}
			return nil
		})
	}
}

// updatesPage lists rush, the harnesses and the sessions' hosts.
func updatesPage() page {
	return page{name: "Updates", form: (*Model).updatesSections, keys: []string{"r", "check again"}}
}

func (m *Model) updatesSections() []section {
	rush := section{title: "rush", rows: []setting{m.rushRow()}}
	harnesses := section{title: "Harnesses", note: "installed → latest"}
	switch {
	case !m.upd.checked:
		harnesses.rows = []setting{{label: "checking", line: func(int) string { return dim("checking what's installed…") }}}
	case len(m.upd.rows) == 0:
		harnesses.rows = []setting{{label: "none", line: func(int) string { return dim("No harness is installed.") }}}
	}
	for _, r := range m.upd.rows {
		harnesses.rows = append(harnesses.rows, m.harnessRow(r))
	}
	hosts := section{title: "Sessions", note: "each runs in a host that stays on the rush it started with", rows: []setting{
		m.hostsRow(),
	}}
	return []section{rush, harnesses, hosts}
}

// updateKey is a row's keys: enter does what the row does, r looks again.
func (m *Model) updateKey(act func() tea.Cmd) func(string) (tea.Cmd, bool) {
	return func(s string) (tea.Cmd, bool) {
		switch s {
		case "enter":
			return act(), true
		case "r":
			m.flash("checking again…", false)
			return m.checkHarnesses(true), true
		}
		return nil, false
	}
}

// updateLine is one row: name, installed, newest and how it stands.
func updateLine(w int, name, have, latest, state string) string {
	// Dates remain in About; keep the version and status readable in the row.
	have, _, _ = strings.Cut(have, " (")
	latest, _, _ = strings.Cut(latest, " (")
	nameW := min(24, max(12, w/4))
	versionW := min(18, max(8, (w-nameW-24)/2))
	line := fit(name, nameW) + " " + fit(have, versionW) + " → " + fit(latest, versionW) + " " + state
	return ansi.Truncate(line, max(0, w), "…")
}

func (m *Model) rushRow() setting {
	have := "dev build"
	if cur, ok := update.Current(); ok {
		have = cur.Short()
	}
	latest := "unknown"
	if m.upd.rushNew.Version != "" {
		latest = m.upd.rushNew.Short()
	}
	var state string
	switch {
	case m.updating:
		state = paint(cYellow, "updating…")
	case m.newer.Version != "":
		latest, state = m.newer.Short(), paint(cYellow, "update available")
	case m.rebuilt:
		state = paint(cYellow, "installed · enter reloads it")
	case have == "dev build" || m.upd.rushNew.Version == "":
		state = dim("unknown")
	default:
		state = paint(cGreen, "up to date")
	}
	return setting{
		label: "rush",
		line:  func(w int) string { return updateLine(w, "rush", have, latest, state) },
		about: func() (string, string, string) {
			return "rush", "rush is installed with go install. Enter installs the newest (#update), then reloads it (#reload): your sessions carry on. Installed: " + have + "; latest: " + latest + ".", ansi.Strip(state)
		},
		key: m.updateKey(func() tea.Cmd {
			switch {
			case m.newer.Version != "":
				return m.installUpdate()
			case m.rebuilt:
				return m.reload()
			}
			m.flash("rush is up to date", false)
			return nil
		}),
		keys: []string{"enter", "update"},
	}
}

func (m *Model) harnessRow(r harnessup.Status) setting {
	state := dim("unknown")
	switch {
	case m.upd.running[r.Kind]:
		state = paint(cYellow, "updating…")
	case r.State() == harnessup.Available:
		state = paint(cYellow, "update available")
	case r.State() == harnessup.UpToDate:
		state = paint(cGreen, "up to date")
	}
	name, latest := agent.HarnessLabel(r.Kind), firstNonEmpty(r.Latest, "unknown")
	how := firstNonEmpty(r.Update.Label, "unknown: update it the way you installed it")
	return setting{
		label: name,
		line:  func(w int) string { return updateLine(w, name, firstNonEmpty(r.Installed, "?"), latest, state) },
		about: func() (string, string, string) {
			return name, "Enter updates it with: " + how + ". It lives at " + tildify(r.Path) + ".", ansi.Strip(state)
		},
		key: m.updateKey(func() tea.Cmd {
			if r.State() != harnessup.Available {
				m.flash(name+" is "+strings.ToLower(ansi.Strip(state)), false)
				return nil
			}
			return m.updateHarness(r)
		}),
		keys: []string{"enter", "update"},
	}
}

func (m *Model) hostsRow() setting {
	n := m.upd.stale
	what := "none"
	if n > 0 {
		what = fmt.Sprintf("%d sessions on an older rush · enter restarts the idle ones", n)
		if n == 1 {
			what = "1 session on an older rush · enter restarts it once idle"
		}
	}
	return setting{
		label: "hosts",
		line:  func(w int) string { return ansi.Truncate(fit("Sessions", 30)+" "+what, max(0, w), "…") },
		about: func() (string, string, string) {
			return "Sessions", "Enter restarts the idle ones on the installed rush now; one in a turn, with something queued or a question waiting, is left until it's over.", what
		},
		key:  m.updateKey(func() tea.Cmd { return m.watchHosts(true) }),
		keys: []string{"enter", "restart the idle ones"},
	}
}
