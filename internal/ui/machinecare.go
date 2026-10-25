package ui

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/sysinfo"
)

// --- Disk and memory: a cleanup agent, offered when they run low ---

// Plain code watches the free disk and the memory pressure. When either
// passes what Settings › Disk and memory says, rush asks whether to start
// a cleanup agent; no agent runs until you say yes. The agent is told
// what rush and Haven know, removes what loses no work, reports, and is
// marked done once its turn ends, which stops it.

const (
	careDiskGB     = 20               // the free disk asked about by default
	careRAMMinutes = 15               // the time under memory pressure asked about by default
	careSnooze     = 6 * time.Hour    // the least time between offers
	careSettle     = 2 * time.Minute  // what a started cleanup agent has before its end counts
	careHavenWait  = 30 * time.Second // what Haven's report may take
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
		at, ok := c.agents[a.ID]
		if !ok || !a.Rush {
			continue
		}
		if a.Done || a.State != "done" || now.Sub(at) < careSettle {
			busy = busy || !a.Done
			continue
		}
		// Its turn is over: done, and its process stopped; its report
		// stays, and a message resumes it.
		delete(c.agents, a.ID)
		m.toggleDone(a)
		m.flash("cleanup finished · its report is in Done", false)
		notifyLater("Cleanup finished", "its report is in rush's Done", true)
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
	if why == "" || m.sheet != nil || m.dialog != nil || m.confirm != nil || m.mode != modeList {
		return nil
	}
	c.asked = now
	// Unasked, and yes starts an agent: an enter meant for the list
	// mustn't pick it.
	m.sheet = &careSheet{why: why, cur: 1}
	notifyLater("rush: "+why, "Start a cleanup agent? rush is asking in its view.", true)
	if time.Since(m.clean.checked) > 30*time.Minute {
		return m.scanWorktrees() // the agent is told what's there, fresh
	}
	return nil
}

// careSheet is Start a cleanup agent, or Not now.
type careSheet struct {
	why  string
	cur  int // 0 Start, 1 Not now
	busy bool
}

func (s *careSheet) width(*Model) int { return 100 }

func (s *careSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Cleanup", s.why, w), ""}
	text := "rush noticed that " + s.why + ". A cleanup agent can look at your worktrees, what agents left in /tmp and Haven's report, " +
		"remove what can go without losing work (a worktree only once it's committed and pushed), stop what's holding memory that nobody works in, " +
		"and tell you what it did. It's marked done when it finishes, which stops it."
	for _, l := range wrap(text, w-2) {
		out = append(out, "  "+l)
	}
	out = append(out, "")
	choices := []struct{ what, about string }{
		{"Start a cleanup agent", "in auto mode, with the agent and model new sessions start with"},
		{"Not now", fmt.Sprintf("asks again in %d hours if it's still so", int(careSnooze.Hours()))},
	}
	for i, c := range choices {
		out = append(out, sheetRow(paint(cText+bold, c.what)+"  "+dim(c.about), i == s.cur, w))
	}
	k := keysFit(w, "↑↓", "choose", "enter", "pick", "esc", "not now")
	if s.busy {
		k = paint(cYellow, "⋯ reading Haven's report")
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
	s.busy = true
	known := m.careKnown()
	return sheetDo(func() (string, error) { return havenReport(), nil }, func(m *Model, haven string, _ error) tea.Cmd {
		if m.sheet != s {
			return nil
		}
		m.sheet = nil
		home, _ := os.UserHomeDir()
		mode := autoMode(agent.Kind(m.nextStart(home).kind))
		return m.startHosted(carePrompt(s.why, known, haven), home, func(c *host.Config) {
			c.SessionID, c.ID = host.NewSessionID()
			c.Name, c.NameFirst = "Cleanup", false
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

// havenReport is what Haven says, when it's installed: its cleanup report
// (machine clean without --yes, as an agent, only reports) and its
// stacks' memory. Off the UI.
func havenReport() string {
	bin, err := exec.LookPath("haven")
	if err != nil {
		return ""
	}
	run := func(args ...string) string {
		ctx, cancel := context.WithTimeout(context.Background(), careHavenWait)
		defer cancel()
		cmd := exec.CommandContext(ctx, bin, args...)
		cmd.Env = append(os.Environ(), "HAVEN_AGENT=1", "NO_COLOR=1")
		out, _ := cmd.CombinedOutput()
		s := strings.TrimSpace(string(out))
		if r := []rune(s); len(r) > 6000 {
			s = string(r[:6000]) + "\n…"
		}
		return s
	}
	return "haven machine clean (its report; nothing was removed):\n" + run("machine", "clean") +
		"\n\nhaven status --json stacks,footprint,focus:\n" + run("status", "--json", "stacks,footprint,focus")
}

// carePrompt is the cleanup agent's task.
func carePrompt(why string, known []string, haven string) string {
	var b strings.Builder
	b.WriteString("Free disk space and memory on this machine, then report and stop. rush started you because " + why + ". ")
	b.WriteString("Nobody is watching, so don't ask questions: do only what loses no work, and leave the rest.\n\n")
	b.WriteString(`Rules:
- Remove a worktree only when it's can-go below, or git shows nothing uncommitted, unpushed or unmerged in it. Use git worktree remove and keep its branch. Never touch one an agent is working in, or a repository's main checkout.
- Remove what agents left in /tmp or outside their projects only when it's marked safe, or nothing has touched it for days and nothing running uses it.
- Build caches that rebuild themselves (node_modules, .next, .turbo, target, dist) may go from worktrees nobody works in.
- Never delete databases, ~/.claude, transcripts, anything in a project that isn't a cache or a worktree, or anything you're unsure of.
`)
	if haven != "" {
		b.WriteString(`- Haven: if its report below ticks things, haven machine clean --yes removes temporary and merged worktrees, cold job scratch and build caches, and never drops databases. For memory, a stack whose worktree nobody works in, that isn't the focus stack, may be stopped with haven down --stack <slug>, which keeps its data. Never haven down --destroy, db reset or db prune.
`)
	}
	b.WriteString("\nEnd with a short report, under 10 lines: what you removed or stopped, how much it freed, and what you left and why.\n\n")
	b.WriteString("What rush knows:\n" + strings.Join(known, "\n") + "\n")
	if haven != "" {
		b.WriteString("\n" + haven + "\n")
	}
	return b.String()
}
