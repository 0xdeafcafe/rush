package convo

import (
	"fmt"
	"maps"
	"slices"
	"strconv"
	"strings"
	"time"
)

// ScriptView is a shell call rush ran as a script, as its trace has it:
// the line it's on and since when, how long each line done took, where
// it stops, and whether it's stopped at one now.
type ScriptView struct {
	At       int // the line running now; 0 when done or not begun
	Since    time.Time
	Took     map[int]time.Duration
	Breaks   map[int]bool
	Held     bool // stopped at a breakpoint, at At
	FailedAt int  // the line a failed script ended on, 0 when it didn't fail
}

// ScriptLineRef is the ref of line n of step ref's script: clicked, it
// sets a breakpoint there or takes it away.
func ScriptLineRef(stepRef string, n int) string { return stepRef + ":line:" + strconv.Itoa(n) }

// ScriptLine is the step ref and line a ScriptLineRef names.
func ScriptLine(ref string) (string, int, bool) {
	i := strings.LastIndex(ref, ":line:")
	if i < 0 {
		return "", 0, false
	}
	n, err := strconv.Atoi(ref[i+len(":line:"):])
	return ref[:i], n, err == nil && n > 0
}

// scriptsSig is what of the scripts changes how a turn draws, as a key.
func scriptsSig(vs map[string]*ScriptView) string {
	if len(vs) == 0 {
		return ""
	}
	ids := make([]string, 0, len(vs))
	for id := range vs {
		ids = append(ids, id)
	}
	slices.Sort(ids)
	var b strings.Builder
	for _, id := range ids {
		v := vs[id]
		fmt.Fprintf(&b, "%s:%d:%t:%d:%d:%d;", id, v.At, v.Held, v.FailedAt, len(v.Breaks), len(v.Took))
		for _, n := range slices.Sorted(maps.Keys(v.Breaks)) {
			fmt.Fprintf(&b, "%d,", n)
		}
	}
	return b.String()
}

// scriptMost is the most lines a script shows; a longer one keeps its
// start, the lines round where it is, its breakpoints and its end.
const scriptMost = 30

// scriptDoc draws a call run as a script: its command as the file it ran
// from, each line numbered, a breakpoint's dot by its number, the line it
// runs now marked with how long it's been on it, and the time each line
// before took.
func (d *drawer) scriptDoc(st *Step, v *ScriptView, cmd string, indent int) {
	pad := d.spine() + blanks(indent-1)
	lines := strings.Split(expandTabs(cmd), "\n")
	numW := len(strconv.Itoa(len(lines)))
	ref := d.ref + ":s:" + st.ID
	live := st.Status == Running
	shown := func(n int) bool {
		return len(lines) <= scriptMost || d.o.Verbose || n <= 6 || n > len(lines)-2 || v.Breaks[n] ||
			v.At > 0 && n >= v.At-3 && n <= v.At+3 || v.FailedAt > 0 && n >= v.FailedAt-3 && n <= v.FailedAt+3
	}
	head := faint("ran as a script")
	switch {
	case v.FailedAt > 0:
		head = faint("ran as a script · ") + paint(cRed, "failed at line "+strconv.Itoa(v.FailedAt))
	case live && !v.Held:
		head = faint("running as a script · click a line's number ahead to stop there")
	}
	if v.Held {
		head = paint(cYellow+bold, "⏸ stopped at line "+strconv.Itoa(v.At)) + faint(" · p goes on")
	}
	d.add(ref, bgWell, pad+head, "")
	var hs hlState
	for i, l := range lines {
		n := i + 1
		if !shown(n) {
			if shown(n - 1) {
				hid := 0
				for j := n; j <= len(lines) && !shown(j); j++ {
					hid++
				}
				d.add(ref, bgWell, pad+blanks(numW+4)+faint(fmt.Sprintf("… %s", plural(hid, "line"))), "")
			}
			highlight(langSh, &hs, l, cSub, nil) // keeps a quote opened above
			continue
		}
		dot := " "
		if v.Breaks[n] {
			dot = paint(cRed, "●")
		}
		num := faint(fmt.Sprintf("%*d", numW, n))
		mark, right := "  ", ""
		switch {
		case n == v.At && v.Held:
			num, mark = paint(cYellow+bold, fmt.Sprintf("%*d", numW, n)), paint(cYellow, "▸ ")
			right = paint(cYellow, "⏸ "+dur(d.o.Now.Sub(v.Since)))
		case n == v.At && live:
			num, mark = paint(cOrange+bold, fmt.Sprintf("%*d", numW, n)), paint(cOrange, "▸ ")
			right = paint(cOrange, d.spin(d.o.Tick)+" "+dur(d.o.Now.Sub(v.Since)))
		case n == v.FailedAt:
			num, mark = paint(cRed+bold, fmt.Sprintf("%*d", numW, n)), paint(cRed, "✗ ")
			if v.Took[n] >= 100*time.Millisecond {
				right = faint(dur(v.Took[n]))
			}
		case v.Took[n] >= 100*time.Millisecond:
			right = faint(dur(v.Took[n]))
		}
		code := highlight(langSh, &hs, l, cSub, nil)
		if n == v.At && (live || v.Held) || n == v.FailedAt {
			code = paint(cText+bold, l)
		}
		d.add(ScriptLineRef(ref, n), bgWell, pad+dot+" "+num+" "+mark+code, right)
	}
}
