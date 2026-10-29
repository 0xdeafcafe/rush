package convo

import (
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// The conversation opens on a card saying what the session is: when it
// started and who started it (you, a plugin, or another agent it works for
// as a subagent), what it runs as and where, and the commands and skills
// it came with. Its start hooks follow, as they always have.

// opening draws the card above the first turn, when it's that turn.
func (d *drawer) opening() {
	s := d.s
	if len(s.Turns) == 0 || s.Turns[0] != d.t {
		return
	}
	in := s.Info
	if in.StartedAt.IsZero() && s.Model == "" {
		return // nothing known of how it began: a transcript read cold
	}
	pad := " " + blanks(gutter-3)
	line := func(ref, s string) { d.add(ref, "", pad+s, "") }
	head := paint(cText+bold, "◆ session")
	if !in.StartedAt.IsZero() {
		head += dim(" · started " + startedAt(in.StartedAt, d.o.Now))
	}
	line("", head+dim(" · ")+d.startedBy())
	var runs []string
	for _, f := range []string{agentLabel(in.Kind), PrettyModel(firstNonEmpty(in.Model, s.Model)), in.Effort, in.PermissionMode, homeDir(firstNonEmpty(in.Cwd, s.Cwd))} {
		if f = strings.TrimSpace(f); f != "" {
			runs = append(runs, f)
		}
	}
	if len(runs) > 0 {
		line("", "  "+dim(strings.Join(runs, " · ")))
	}
	if d.o.Hosted {
		d.added()
	}
	if n := len(s.Commands); n > 0 {
		ref := d.ref + ":opening:skills"
		if !d.o.Open[ref] {
			line(ref, "  "+dim(plural(n, "command")+" and skills ")+faint("▸ list"))
		} else {
			line(ref, "  "+dim(plural(n, "command")+" and skills ")+faint("▾ hide"))
			names := make([]string, 0, n)
			for _, c := range s.Commands {
				names = append(names, "/"+c.Name)
			}
			for _, r := range wrap(dim(strings.Join(names, "  ")), max(10, min(d.cw-gutter-3, d.o.proseCap()))) {
				line(ref, "    "+r)
			}
		}
	}
	d.add("", "", "", "")
}

// added draws what was added to the agent's prompt: rush's own, and the
// brief the session was started with, opened to read it whole.
func (d *drawer) added() {
	pad := " " + blanks(gutter-3)
	b := d.o.Brief
	if b == "" {
		d.add("", "", pad+"  "+dim("prompt: rush's own"), "")
		return
	}
	ref := d.ref + ":opening:brief"
	n := strings.Count(b, "\n") + 1
	if !d.o.Open[ref] {
		d.add(ref, "", pad+"  "+dim("prompt: rush's own, and its brief · "+plural(n, "line")+" ")+faint("▸ show"), "")
		return
	}
	d.add(ref, "", pad+"  "+dim("prompt: rush's own, and its brief ")+faint("▾ hide"), "")
	w := max(10, min(d.cw-gutter-3, d.o.proseCap()))
	for _, r := range d.cachedRows(b, "brief", w, func() []string { return wrap(paint(cSub, b), w) }) {
		d.add(ref, "", pad+"    "+r, "")
	}
}

// startedBy says who began the session.
func (d *drawer) startedBy() string {
	in := d.s.Info
	switch {
	case d.o.Parent != "":
		return dim("by ") + paint(cText, d.o.Parent) + dim(", as its subagent")
	case in.Meta["spawnedBy"] != "":
		return dim("by another session, as its subagent")
	case d.o.StartedFrom != "":
		return dim("by ") + paint(cText, d.o.StartedFrom)
	case in.Meta["startedFrom"] != "":
		return dim("by another session")
	case in.StartedBy != "":
		return dim("by the ") + paint(cText, in.StartedBy) + dim(" plugin")
	case d.t.From != "":
		return dim("by ") + paint(cText, d.t.From)
	}
	return dim("by you")
}

// startedAt is when, as you'd say it: the time today, else the day too.
func startedAt(t, now time.Time) string {
	t = t.Local()
	if now.IsZero() {
		now = time.Now()
	}
	if y, m, dd := t.Date(); func() bool { ny, nm, nd := now.Local().Date(); return y == ny && m == nm && dd == nd }() {
		return t.Format("15:04")
	}
	return t.Format("Mon 2 Jan 15:04")
}

// agentLabel names the harness a session runs: empty is Claude Code.
func agentLabel(kind string) string {
	return agent.HarnessLabel(agent.Migrated(kind))
}

// homeDir is p with your home as ~.
func homeDir(p string) string {
	if home, err := os.UserHomeDir(); err == nil && home != "" && strings.HasPrefix(p, home) {
		return "~" + p[len(home):]
	}
	return p
}

// openingPrint is what the card reads, for the first turn's cache key.
func (s *Session) openingPrint(o Options) string {
	in := s.Info
	return strconv.FormatInt(in.StartedAt.Unix(), 10) + "|" + in.Model + s.Model + "|" + in.Effort + "|" + in.PermissionMode + "|" +
		in.Kind + "|" + in.Cwd + s.Cwd + "|" + in.StartedBy + in.Meta["spawnedBy"] + in.Meta["startedFrom"] + "|" + o.Parent + o.StartedFrom + "|" + strconv.FormatBool(o.Hosted) + strconv.Itoa(len(o.Brief)) + "|" + strconv.Itoa(len(s.Commands)) + "|" + o.Now.Local().Format("2006-01-02")
}
