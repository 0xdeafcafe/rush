package ui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/netwatch"
)

// --- the network ---

// netMsg is the API coming back, or the machine joining another network.
type netMsg struct{}

// watchNet starts netwatch and waits for the network to change. Offline
// (--soak) never does.
func (m *Model) watchNet() tea.Cmd {
	if m.offline {
		return nil
	}
	netwatch.Start()
	ch := netwatch.Changes()
	return func() tea.Msg {
		<-ch
		return netMsg{}
	}
}

// onNet asks again at once for what waited on the network, rather than at
// the next minute's fetch, and goes on waiting.
func (m *Model) onNet() tea.Cmd {
	cmds := []tea.Cmd{m.watchNet()}
	s := netwatch.Now()
	events.Log("network-changed", map[string]any{"up": s.Up, "known": s.Known, "target": s.Target, "why": s.Why, "refetch": !netwatch.Down()})
	if !netwatch.Down() {
		cmds = append(cmds, m.fetchUsage(), m.findLogins(), m.fetchQuotas())
	}
	return tea.Batch(cmds...)
}

// netRate is bytes a second, short: 0B, 40K, 1.2M.
func netRate(b float64) string {
	switch {
	case b >= 1<<30:
		return fmt.Sprintf("%.1fG", b/(1<<30))
	case b >= 10<<20:
		return fmt.Sprintf("%.0fM", b/(1<<20))
	case b >= 1<<20:
		return fmt.Sprintf("%.1fM", b/(1<<20))
	case b >= 1<<10:
		return fmt.Sprintf("%.0fK", b/(1<<10))
	}
	return fmt.Sprintf("%.0fB", b)
}

// netSeg is the top bar's network: whether it's been steady. Red while
// the API can't be reached, yellow while it's recently failed, been slow,
// or changed.
func netSeg() string {
	s := netwatch.Now()
	switch {
	case !s.Known:
		return ""
	case !s.Up:
		return paint(cRed, "offline")
	case !s.Steady:
		return paint(cYellow, "net shaky")
	}
	return dim("net steady")
}

// netSheet is #network: whether the API answers, the network rush is on,
// what of rush's waits for it, and the sessions it stopped.
type netSheet struct{}

func (*netSheet) width(*Model) int { return 96 }

func (*netSheet) key(m *Model, k tea.KeyPressMsg, s string) tea.Cmd {
	switch s {
	case "c", "r":
		netwatch.Check()
	case "esc", "q", "enter", "ctrl+c":
		m.sheet = nil
	}
	return nil
}

func (*netSheet) body(m *Model, w, h int) []string {
	s := netwatch.Now()
	now := time.Now()
	label := func(l string) string { return paint(cSub, fmt.Sprintf("%-13s", l)) }
	out := []string{sheetTitle("Network", "whether the API answers, the network rush is on, and what waits for it", w), ""}

	// The API.
	var api string
	switch {
	case m.offline:
		api = dim("not checked · --soak never uses the network")
	case !s.Known:
		api = paint(cYellow, "● checking") + dim(" · "+s.Target)
	case s.Up:
		api = paint(cGreen, "● answers") + dim(fmt.Sprintf(" · %s · connects in %s · up %s", s.Target, s.Latency.Round(time.Millisecond), dur(now.Sub(s.Since))))
		if s.Latency > time.Second {
			api = paint(cYellow, "● slow") + dim(fmt.Sprintf(" · %s · connects in %s", s.Target, s.Latency.Round(time.Millisecond)))
		}
	default:
		api = paint(cRed, "● can't be reached") + dim(fmt.Sprintf(" · %s · %s · down %s", s.Target, s.Why, dur(now.Sub(s.Since))))
	}
	out = append(out, label("API")+api)
	if s.Known {
		next := "every 2m while it's steady"
		switch {
		case !s.Up:
			next = "every 5–30s until it answers"
		case !s.Steady:
			next = "every 30s until it's steady"
		}
		checking := ""
		if s.Checking {
			checking = " · checking now"
		}
		out = append(out, label("")+faint(fmt.Sprintf("checked %s ago · %d checks · %s%s", age(now.Sub(s.Checked)), s.Checks, next, checking)))
	}

	// Whether it's been steady.
	steady := paint(cGreen, "● steady") + dim(" · every check answered, promptly, on one network over the last "+dur(s.Window))
	switch {
	case !s.Known:
		steady = faint("–")
	case !s.Steady:
		steady = paint(cYellow, "● shaky") + dim(" · over the last "+dur(s.Window)+": "+strings.Join(s.Shaky, " · "))
	}
	out = append(out, label("Steadiness")+steady)

	// The network, and how fast it moves.
	speed := faint("–")
	if s.Rated {
		speed = paint(cText, "↓"+netRate(s.RxRate)+"/s  ↑"+netRate(s.TxRate)+"/s")
	}
	out = append(out, "", label("Network")+paint(cText, orDash(s.Network))+dim("  ·  ")+speed)

	// Noticing another network.
	switch s.Changes {
	case 0:
		out = append(out, label("New network")+dim("none since rush opened")+faint(" · interfaces looked at every 5s; a change checks the API at once"))
	default:
		n := fmt.Sprintf("%d change%s · last %s ago", s.Changes, plural(s.Changes), age(now.Sub(s.Changed)))
		how := paint(cYellow, "checking the API on it…")
		if s.Noticed > 0 {
			how = paint(cGreen, "API checked "+s.Noticed.Round(10*time.Millisecond).String()+" after it changed")
		}
		out = append(out, label("New network")+dim(n+" · ")+how)
	}

	// rush's own jobs.
	out = append(out, "", paint(cText+bold, "rush's jobs that use it"))
	if len(s.Jobs) == 0 {
		out = append(out, "  "+faint("none has run yet"))
	}
	for _, j := range s.Jobs {
		var mark, what string
		switch {
		case j.Paused:
			mark = paint(cYellow, "⏸")
			what = paint(cYellow, fmt.Sprintf("waiting for the network · held back %d×", j.Skipped))
		case j.Err != "":
			mark = paint(cRed, "✗")
			what = paint(cRed, "failed: "+oneLine(j.Err))
		case j.Runs > 0:
			what = dim(fmt.Sprintf("ran %d× · last %s ago", j.Runs, age(now.Sub(j.Last))))
		default:
			what = dim("not run yet")
		}
		if mark == "" {
			mark = paint(cGreen, "✓")
		}
		out = append(out, ansi.Truncate("  "+mark+" "+paint(cText, fmt.Sprintf("%-22s", j.Name))+what, w, "…"))
	}

	// Sessions it stopped.
	out = append(out, "", paint(cText+bold, "Sessions waiting on it"))
	waiting := 0
	if m.snap != nil {
		for _, a := range m.snap.Agents {
			why, ok := netWaiting(a, now)
			if !ok {
				continue
			}
			waiting++
			out = append(out, ansi.Truncate("  "+paint(cYellow, "⟳")+" "+paint(cText, fmt.Sprintf("%-22s", ansi.Truncate(a.DisplayName, 22, "…")))+dim(why), w, "…"))
		}
	}
	if waiting == 0 {
		out = append(out, "  "+faint("none"))
	}

	// The UI's own goroutine.
	n, longest, last, path := stallReport()
	out = append(out, "", paint(cText+bold, "Keys and frames"))
	if n == 0 {
		out = append(out, "  "+paint(cGreen, "✓")+" "+dim(fmt.Sprintf("nothing has held the UI up over %s since rush opened", stallAfter)))
	} else {
		out = append(out, "  "+paint(cYellow, "!")+" "+paint(cText, fmt.Sprintf("held up %d× over %s", n, stallAfter))+
			dim(fmt.Sprintf(" · longest %s by %s%s · last %s ago", longest.took.Round(time.Millisecond), longest.what, inWhere(longest.where), age(now.Sub(last.at)))),
			"    "+faint("what it was doing: "+tildify(path)))
	}

	out = append(out, "", keysFit(w, "c", "check now", "esc", "close"))
	if len(out) > h {
		out = out[:h]
	}
	return out
}

// netWaiting is whether an agent waits on the network or the API, and why.
func netWaiting(a *fleet.Agent, now time.Time) (string, bool) {
	switch {
	case strings.HasPrefix(a.Detail, "offline"):
		return a.Detail, true
	case a.Halted() && a.Continues(now):
		return a.HaltReason(), true
	}
	return "", false
}

func inWhere(w string) string {
	if w == "" || w == "?" {
		return ""
	}
	return " in " + w
}

func orDash(s string) string {
	if s == "" {
		return "–"
	}
	return s
}
