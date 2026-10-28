package ui

import (
	"fmt"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// A tool call waiting for your yes gets the same framed card as a memory
// note: what it asks in the top edge, with where it runs and how many
// wait; the command, the file's change or the address in the middle; and
// buttons to allow it once, always, switch the session to don't-ask, or
// deny it. y, a, o and n still press them; ←→ pick one and enter presses
// it; a click presses the one under it.

// permBtns are the permission card's buttons for st, w wide at most.
func permBtns(c *hostConn, st *convo.Step, w int) [][3]string {
	bs := [][3]string{{"y", "y", "Allow once"}}
	if st.Approval != nil && st.Approval.Always {
		bs = append(bs, [3]string{"a", "a", alwaysLabel(st.Approval.AlwaysAs)})
	}
	if mode := bypassMode(sessionAgent(c), c); mode != "" && c.sess.Info.PermissionMode != mode {
		bs = append(bs, [3]string{"o", "o", "Yolo"})
	}
	bs = append(bs, [3]string{"n", "n", "Deny"})
	// Too wide, and always allow's rule goes first.
	width := func() int {
		n := 0
		for _, b := range bs {
			n += cellw.String(b[1]+" "+b[2]) + 7
		}
		return n
	}
	for i, b := range bs {
		if over := width() - w; b[0] == "a" && over > 0 {
			bs[i][2] = cellw.Truncate(b[2], max(len("Always allow"), cellw.String(b[2])-over), "…")
		}
	}
	return bs
}

// alwaysLabel is always allow's button: "Always allow Bash(pnpm lint:*)".
func alwaysLabel(as string) string {
	w := alwaysWords(as)
	return strings.ToUpper(w[:1]) + w[1:]
}

// permTitle asks what the card is for.
func permTitle(st *convo.Step) string {
	switch st.Call().Kind {
	case tool.Shell:
		return "Run this command?"
	case tool.Edit, tool.Write, tool.Notebook:
		return "Change this file?"
	case tool.Fetch, tool.WebSearch:
		return "Go online?"
	}
	return "Use " + st.Tool + "?"
}

// permissionCard draws the card for the tool call p[0] waits on, w wide
// and in at most maxH rows, and the buttons on it.
func (m *Model) permissionCard(a *fleet.Agent, c *hostConn, p []*convo.Step, w, maxH int) ([]string, []cardBtn) {
	st := p[0]
	edge := cYellow
	if c.cardFocus {
		edge = cOrange
	}
	e := func(s string) string { return paint(edge, s) }
	var out []string
	row := func(s string) { out = append(out, onBg(qCard, e("│")+fit("  "+s, w-2)+e("│"), w)) }
	tw := max(20, w-6)

	// The top edge: what it asks, then whose it is, where it runs and how
	// many wait.
	left := e("╭─ ") + paint(cYellow, "●") + " " + paint(cText+bold, permTitle(st)) + " "
	var meta []string
	if under := st.Under(); under != nil {
		// A subagent's: say whose, since it's not the agent you're talking to.
		who := "a subagent"
		for _, sa := range c.subs {
			if sa.ToolUseID == under.ID {
				who = strings.TrimSpace(sa.Type + " · " + oneLine(sa.Description))
			}
		}
		meta = append(meta, paint(cBlue, "⇉ from "+who))
	}
	if st.Call().Kind == tool.Shell && a.Cwd != "" {
		meta = append(meta, paint(cSub, "in "+shortDir(tildify(a.Cwd), 36)))
	}
	if len(p) > 1 {
		meta = append(meta, paint(cSub, fmt.Sprintf("1 of %d", len(p))))
	}
	right := ""
	if len(meta) > 0 {
		right = " " + strings.Join(meta, dim(" · ")) + " "
		if room := w - cellw.String(left) - 4; cellw.String(right) > room {
			right = cellw.Truncate(right, max(0, room), "… ")
		}
	}
	fill := w - cellw.String(left) - cellw.String(right) - 2
	out = append(out, onBg(qCard, left+e(strings.Repeat("─", max(1, fill)))+right+e("─╮"), w))
	row("")

	// What it does: it keeps to maxH, the body giving way, the rest (9
	// rows) not.
	body := approvalBody(st, a.Cwd, tw)
	if room := max(3, maxH-9); maxH > 0 && len(body) > room {
		body = append(body[:room-1:room-1], dim(fmt.Sprintf("  … %d more lines · ctrl+o shows all", len(body)-room+1)))
	}
	for _, l := range body {
		row(l)
	}
	row("")

	bs := permBtns(c, st, w-6)
	pick := min(c.btnPick, len(bs)-1)
	rows, btns := cardButtons(bs, c.cardFocus, pick, c.subHover, len(out))
	for _, r := range rows {
		row(r)
	}

	// The bottom edge says how to get to it, or back from it, and that
	// what you type is why not.
	hint := " ↑ to answer · or type why not and enter "
	if c.cardFocus {
		hint = " ←→ pick · enter · esc back to typing "
	}
	out = append(out, onBg(qCard, e("╰"+strings.Repeat("─", max(1, w-3-cellw.String(hint))))+dim(hint)+e("─╯"), w))
	return out, btns
}

// shellLines is a command a step per line, so a chain reads down rather
// than wrapping across: it breaks after &&, ||, ; and | outside quotes and
// brackets, and at the command's own line breaks.
func shellLines(cmd string) []string {
	var out []string
	var cur strings.Builder
	var quote rune
	depth := 0
	flush := func() {
		if l := strings.TrimSpace(cur.String()); l != "" {
			out = append(out, l)
		}
		cur.Reset()
	}
	rs := []rune(cmd)
	for i := 0; i < len(rs); i++ {
		r := rs[i]
		if r == '\\' && quote != '\'' && i+1 < len(rs) && rs[i+1] == '\n' {
			i++
			continue // a continued line goes on as one
		}
		cur.WriteRune(r)
		switch {
		case r == '\\' && quote != '\'' && i+1 < len(rs):
			i++
			cur.WriteRune(rs[i])
		case quote != 0:
			if r == quote {
				quote = 0
			}
		case r == '\'' || r == '"' || r == '`':
			quote = r
		case r == '(' || r == '{':
			depth++
		case (r == ')' || r == '}') && depth > 0:
			depth--
		case r == '\n':
			flush()
		case depth > 0:
		case (r == '&' || r == '|') && i+1 < len(rs) && rs[i+1] == r:
			cur.WriteRune(rs[i+1])
			i++
			flush()
		case r == ';' || r == '|' && (i == 0 || rs[i-1] != '>'):
			flush()
		}
	}
	flush()
	return out
}
