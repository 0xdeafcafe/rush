package ui

import (
	"cmp"
	"context"
	"fmt"
	"os"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	careplugin "github.com/0xdeafcafe/rush/internal/bundled/cleanup"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/sysinfo"
)

// --- Disk and memory: a cleanup agent, offered when they run low ---

// Plain code watches the free disk and the memory pressure. When either
// passes what Settings › Disk and memory says, rush asks whether to start
// a cleanup agent; no agent runs until you say yes. The agent is told
// what rush and Haven know, removes what loses no work, reports, and is
// marked done once its turn ends, which stops it.

const (
	careDiskGB     = 20              // the free disk asked about by default
	careRAMMinutes = 15              // the time under memory pressure asked about by default
	careSnooze     = 6 * time.Hour   // the least time between offers
	careSettle     = 2 * time.Minute // what a started cleanup agent has before its end counts
)

// machineCare is what the watch remembers.
type machineCare struct {
	pressedSince time.Time            // when the memory came under pressure; zero while it isn't
	asked        time.Time            // when rush last offered
	agents       map[string]time.Time // cleanup sessions this view started, by host id, and when
}

// careReason is why a cleanup is worth offering, in words, or "" when it
// isn't: free disk under diskGB (0 the default, -1 never), or the memory
// under pressure for longer than ramMin minutes (the same).
func careReason(free, total uint64, diskGB int, pressed time.Duration, ramMin int) string {
	if diskGB == 0 {
		diskGB = careDiskGB
	}
	if ramMin == 0 {
		ramMin = careRAMMinutes
	}
	if diskGB > 0 && total > 0 && free < uint64(diskGB)<<30 {
		return fmt.Sprintf("the disk has %s free, under the %dG you set", disk(int64(free)), diskGB)
	}
	if ramMin > 0 && pressed >= time.Duration(ramMin)*time.Minute {
		return fmt.Sprintf("the memory has been under pressure for %d minutes", int(pressed.Minutes()))
	}
	return ""
}

// watchMachine runs each tick in the view that leads: it follows the
// memory pressure, ends cleanup agents whose turn is over, and offers a
// cleanup once the screen is free.
func (m *Model) watchMachine() tea.Cmd {
	c := &m.care
	now := time.Now()
	if m.tick%5 == 0 {
		if gate.Pressure(0) >= gate.PressureWarn {
			if c.pressedSince.IsZero() {
				c.pressedSince = now
			}
		} else {
			c.pressedSince = time.Time{}
		}
	}
	busy, stops := false, []tea.Cmd(nil)
	for _, a := range m.snap.Agents {
		if !a.Rush || a.StartedBy != careplugin.Name || a.Done {
			continue
		}
		at, ok := c.agents[a.ID]
		if !ok {
			if a.State == "done" {
				continue // finished before a reload, or brought back by you: yours now
			}
			// Started before a reload, and still at it: watched from now.
			if c.agents == nil {
				c.agents = map[string]time.Time{}
			}
			at, c.agents[a.ID] = now, now
		}
		if a.State != "done" || now.Sub(at) < careSettle {
			busy = true
			continue
		}
		// Its turn is over: done, and its process stopped; its report
		// stays, and a message resumes it.
		delete(c.agents, a.ID)
		m.toggleDone(a)
		m.flash("cleanup finished · its report is in Earlier", false)
		notifyLater("Cleanup finished", "its report is in rush's Earlier", true)
		id := a.ID
		stops = append(stops, cmdErr("", func() error {
			cl, err := host.Dial(id)
			if err != nil {
				return nil // already gone
			}
			defer cl.Close()
			return cl.Stop()
		}))
	}
	if len(stops) > 0 || busy || m.hosted != "" || m.tick < 120 || now.Sub(c.asked) < careSnooze {
		return tea.Batch(stops...)
	}
	cfg := m.store.Config
	var pressed time.Duration
	if !c.pressedSince.IsZero() {
		pressed = now.Sub(c.pressedSince)
	}
	_, d := sysinfo.Now()
	why := careReason(d.Free, d.Total, cfg.CleanupDiskGB, pressed, cfg.CleanupRAMMinutes)
	short := "memory under pressure"
	if strings.HasPrefix(why, "the disk") {
		short = disk(int64(d.Free)) + " free on disk"
	}
	if why == "" || m.sheet != nil || m.dialog != nil || m.confirm != nil || m.mode != modeList {
		return nil
	}
	c.asked = now
	// Whether the cleanup plugin is on, and who reports, are on disk.
	return sheetDo(func() (careOffer, error) {
		return careOffer{on: careplugin.On(), from: plugin.Providers(plugin.ServiceCleanupReport)}, nil
	}, func(m *Model, o careOffer, _ error) tea.Cmd {
		if m.sheet != nil || m.dialog != nil || m.confirm != nil || m.mode != modeList {
			return nil
		}
		// Unasked, and yes starts an agent or turns a plugin on: an enter
		// meant for the list mustn't pick it.
		m.sheet = &careSheet{why: why, short: short, cur: 1, off: !o.on, from: o.from}
		if o.on {
			notifyLater("rush: "+why, "Start a cleanup agent? rush is asking in its view.", true)
		} else {
			notifyLater("rush: "+why, "Turn on the cleanup plugin? rush is asking in its view.", true)
		}
		if time.Since(m.clean.checked) > 30*time.Minute {
			return m.scanWorktrees() // the agent is told what's there, fresh
		}
		return nil
	})
}

// careOffer is what an offer needs from disk: whether the cleanup plugin
// is on, and the plugins that report to it.
type careOffer struct {
	on   bool
	from []string
}

// careSheet is Start a cleanup agent, or Not now; with the cleanup plugin
// off, it recommends turning it on first.
type careSheet struct {
	why string
	// short is why in a few words, for the agent's row: "18G free on disk".
	short string
	cur   int // 0 Start (or Turn on), 1 Not now
	busy  bool
	// looking is rush checking the worktrees before it starts the agent.
	looking bool
	// off is the cleanup plugin being off: the sheet offers turning it on.
	off bool
	// from are the plugins that report to the cleanup agent.
	from []string
}

// reads is what the cleanup agent reads, in words.
func (s *careSheet) reads() string {
	what := "your worktrees, what agents left in /tmp"
	if len(s.from) == 0 {
		return what
	}
	return what + " and what " + andList(s.from) + " report"
}

// andList is "a", "a and b", "a, b and c".
func andList(l []string) string {
	if len(l) < 2 {
		return strings.Join(l, "")
	}
	return strings.Join(l[:len(l)-1], ", ") + " and " + l[len(l)-1]
}

func (s *careSheet) width(*Model) int { return 100 }

func (s *careSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Cleanup", s.why, w), ""}
	text := "rush noticed that " + s.why + ". A cleanup agent can look at " + s.reads() + ", " +
		"remove what can go without losing work (a worktree only once it's committed and pushed), stop what's holding memory that nobody works in, " +
		"and tell you what it did. It's marked done when it finishes, which stops it."
	first := struct{ what, about string }{"Start a cleanup agent", "in auto mode, with the agent and model new sessions start with"}
	if s.off {
		text = "rush noticed that " + s.why + ". The cleanup plugin is off, and recommended now: with it on, rush offers a cleanup agent " +
			"when this happens, which looks at " + s.reads() + ", removes what can go without losing work and tells you what it did. " +
			"Nothing starts until you say yes."
		first = struct{ what, about string }{"Turn on the cleanup plugin", "recommended · then asks whether to start a cleanup agent now"}
	}
	for _, l := range wrap(text, w-2) {
		out = append(out, "  "+l)
	}
	out = append(out, "")
	choices := []struct{ what, about string }{
		first,
		{"Not now", fmt.Sprintf("asks again in %d hours if it's still so", int(careSnooze.Hours()))},
	}
	for i, c := range choices {
		out = append(out, sheetRow(paint(cText+bold, c.what)+"  "+dim(c.about), i == s.cur, w))
	}
	k := keysFit(w, "↑↓", "choose", "enter", "pick", "esc", "not now")
	if s.busy {
		var doing []string
		if s.looking {
			doing = append(doing, "looking at your worktrees")
		}
		if len(s.from) > 0 {
			doing = append(doing, "asking "+andList(s.from)+" for their reports")
		}
		k = paint(cYellow, "⋯ "+cmp.Or(andList(doing), "starting"))
	}
	return append(out, "", k)
}

func (s *careSheet) key(m *Model, k tea.KeyPressMsg, str string) tea.Cmd {
	if s.busy {
		return nil
	}
	switch str {
	case "up", "shift+tab", "down", "tab":
		s.cur = 1 - s.cur
	case "y":
		s.cur = 0
		return s.pick(m)
	case "n", "esc", "ctrl+c":
		s.cur = 1
		return s.pick(m)
	case "enter":
		return s.pick(m)
	}
	return nil
}

func (s *careSheet) pick(m *Model) tea.Cmd {
	if s.cur == 1 {
		m.sheet = nil
		m.flash(fmt.Sprintf("no cleanup · asks again in %d hours", int(careSnooze.Hours())), false)
		return nil
	}
	if s.off {
		s.busy = true
		return sheetDo(func() (struct{}, error) {
			err := plugin.SetBundled(careplugin.Name, true)
			if err == nil {
				err = hooks.Reload()
			}
			return struct{}{}, err
		}, func(m *Model, _ struct{}, err error) tea.Cmd {
			if m.sheet != s {
				return nil
			}
			s.busy = false
			if err != nil {
				m.sheet = nil
				m.flash("couldn't turn on the cleanup plugin: "+err.Error(), true)
				return nil
			}
			s.off, s.cur = false, 1
			m.flash("cleanup plugin on", false)
			return nil
		})
	}
	s.busy = true
	// The agent is told what rush's own check of the worktrees says, which
	// knows a squash-merged branch by its pull request; git alone doesn't,
	// and an agent left to look for itself spends its time finding them.
	var scan func() worktreesMsg
	if m.clean.checked.IsZero() || time.Since(m.clean.checked) > 30*time.Minute {
		s.looking = true
		scan = m.worktreeScan()
	}
	return sheetDo(func() (careStart, error) {
		var st careStart
		done := make(chan struct{})
		go func() {
			defer close(done)
			if scan != nil {
				w := scan()
				st.scan = &w
			}
		}()
		st.reports = careReports()
		<-done
		return st, nil
	}, func(m *Model, st careStart, _ error) tea.Cmd {
		if m.sheet != s {
			return nil
		}
		m.sheet = nil
		if st.scan != nil {
			m.onWorktrees(*st.scan)
		}
		home, _ := os.UserHomeDir()
		mode := autoMode(agent.Kind(m.nextStart(home).kind))
		return m.startHosted(carePrompt(s.why, m.careKnown(), st.reports), home, func(c *host.Config) {
			c.SessionID, c.ID = host.NewSessionID()
			c.Name, c.NameFirst = "Cleanup", false
			// rush's own: the list says so, and finds it again after a reload.
			c.StartedBy, c.Meta = careplugin.Name, map[string]string{"why": s.short}
			if mode != "" {
				c.PermissionMode = mode // nobody is watching to answer its permission prompts
			}
			if m.care.agents == nil {
				m.care.agents = map[string]time.Time{}
			}
			m.care.agents[c.ID] = time.Now()
		})
	})
}

// careStart is what a cleanup agent is started with: a fresh look at the
// worktrees when rush's is old, and the plugins' reports.
type careStart struct {
	scan    *worktreesMsg
	reports []plugin.CleanupReport
}

// autoMode is agent k's auto mode, which runs without asking about safe
// actions, or "" when it has none: its own setting stands then.
func autoMode(k agent.Kind) string {
	c, _ := agent.ChoicesOf(k)
	for _, x := range c.Modes {
		if x.ID == "auto" {
			return x.ID
		}
	}
	return ""
}

// careKnown is what rush knows of the disk, the memory, worktrees and
// what agents left, as lines for the cleanup agent.
func (m *Model) careKnown() []string {
	_, d := sysinfo.Now()
	out := []string{fmt.Sprintf("Disk: %s free of %s.", disk(int64(d.Free)), disk(int64(d.Total)))}
	if lvl := gate.MemoryLevel(); lvl >= 0 {
		out = append(out, fmt.Sprintf("Memory: %d%% available.", lvl))
	}
	if m.clean.checked.IsZero() {
		return append(out, "Worktrees: rush hasn't looked yet; find them with git worktree list in the repositories under ~/Source.")
	}
	out = append(out, "", "Worktrees, as rush checked them "+ago(time.Since(m.clean.checked))+" (path · branch · size · verdict):")
	for _, w := range m.clean.wts {
		l := fmt.Sprintf("- %s · %s · %s · %s", w.Path, w.Branch, disk(w.Size), w.Verdict().Kind)
		if r := w.Verdict().Reason; r != "" {
			l += " (" + r + ")"
		}
		if a := m.running(w.Agents); a != nil {
			l += " · an agent is working in it now"
		}
		out = append(out, l)
	}
	var left []string
	for _, l := range m.clean.outside {
		if l.Gone {
			continue
		}
		s := fmt.Sprintf("- %s · %s", l.Path, disk(l.Size))
		if l.Safe() {
			s += " · safe to remove"
		} else if why := l.Why(); why != "" {
			s += " · keep: " + why
		}
		left = append(left, s)
	}
	if len(left) > 0 {
		out = append(out, "", "What agents left outside their projects:")
		out = append(out, left...)
	}
	if t := m.clean.tmp; t.StaleItems > 0 {
		out = append(out, "", fmt.Sprintf("/tmp: %d of your items untouched for days, %s.", t.StaleItems, disk(t.Stale)))
	}
	return out
}

// careReports asks every plugin that provides cleanup.report for what it
// knows. One that can't answer is left out. Off the UI.
func careReports() []plugin.CleanupReport {
	ctx, cancel := context.WithTimeout(context.Background(), plugin.ServiceTimeout+5*time.Second)
	defer cancel()
	answers, _ := plugin.AskService(ctx, plugin.ServiceCleanupReport)
	var out []plugin.CleanupReport
	for _, a := range answers {
		var r plugin.CleanupReport
		if a.Error != "" || jsonx.Unmarshal(a.Result, &r) != nil || strings.TrimSpace(r.Report) == "" {
			continue
		}
		r.Title = cmp.Or(r.Title, a.Plugin)
		out = append(out, r)
	}
	return out
}

// carePrompt is the cleanup agent's task.
func carePrompt(why string, known []string, reports []plugin.CleanupReport) string {
	var b strings.Builder
	b.WriteString("Free disk space and memory on this machine, then report and stop. rush started you because " + why + ". ")
	b.WriteString("Nobody is watching, so don't ask questions: do only what loses no work, and leave the rest.\n\n")
	b.WriteString(`Rules:
- Remove a worktree only when it's can-go below, or git shows nothing uncommitted, unpushed or unmerged in it. can-go is safe even when git calls its branch unmerged: rush found its pull request merged (a squash merge). Use git worktree remove and keep its branch. Never touch one an agent is working in, or a repository's main checkout.
- Remove what agents left in /tmp or outside their projects only when it's marked safe, or nothing has touched it for days and nothing running uses it.
- Build caches that rebuild themselves (node_modules, .next, .turbo, target, dist) may go from worktrees nobody works in.
- pnpm's node_modules are mostly hard links into its shared store, so removing them frees little until pnpm store prune runs; run it once after.
- Never delete databases, ~/.claude, transcripts, anything in a project that isn't a cache or a worktree, or anything you're unsure of.
`)
	for _, r := range reports {
		if r.Rules != "" {
			b.WriteString("\n" + r.Title + ", as its plugin says:\n" + strings.TrimSpace(r.Rules) + "\n")
		}
	}
	b.WriteString("\nDo the biggest wins first. Wait on a long command by running it in the foreground with a long timeout, or in the background until its notice comes; never sleep to wait.\n")
	b.WriteString("\nEnd with a short report, under 10 lines: what you removed or stopped, how much it freed as df measures it before and after, and what you left and why.\n\n")
	b.WriteString("What rush knows:\n" + strings.Join(known, "\n") + "\n")
	for _, r := range reports {
		b.WriteString("\nWhat " + r.Title + " reports:\n" + strings.TrimSpace(r.Report) + "\n")
	}
	return b.String()
}
