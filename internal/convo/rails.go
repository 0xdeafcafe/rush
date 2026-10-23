package convo

import (
	"strings"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
)

// railed draws a group of rows with draw, then runs a rail down column col
// beside them, so you can see which rows the group owns: ╭ on its header
// (when it has one among the rows), │ down the rest, ╰ on the last. Every
// group's rail is the same dim colour; a group inside one adds its own a
// column further in.
func (d *drawer) railed(col int, header bool, draw func()) {
	from := len(d.lines)
	draw()
	end := len(d.lines)
	for end > from && d.isBlank(end-1) {
		end-- // a gap after the group isn't in it
	}
	for i := from; i < end; i++ {
		g := "│"
		switch {
		case i == from && header:
			g = "╭"
		case i == end-1:
			g = "╰"
		}
		d.lines[i].Text = putRail(d.lines[i].Text, col, g)
	}
	if header && from < end && d.lines[from].Ref != "" {
		d.own(d.lines[from].Ref, from)
	}
}

// OwnerSep joins the refs in a row's Owner.
const OwnerSep = "\x00"

// own makes the rows drawn since from part of ref, for the mouse to light
// together: those with no owner are ref's, the rest's owners are inside it.
// A gap after them isn't theirs.
func (d *drawer) own(ref string, from int) {
	end := len(d.lines)
	for end > from && d.isBlank(end-1) {
		end--
	}
	for i := from; i < end; i++ {
		switch o := d.lines[i].Owner; {
		case o == "":
			d.lines[i].Owner = ref
		case o != ref && !strings.HasPrefix(o, ref+OwnerSep):
			d.lines[i].Owner = ref + OwnerSep + o
		}
	}
}

// putRail sets the cell at column col of a drawn row to the rail's glyph,
// keeping what the row's own colours and ground do either side of it.
func putRail(s string, col int, g string) string {
	if w := cellw.String(s); w < col+1 {
		s += blanks(col + 1 - w)
	}
	return ansi.Truncate(s, col, "") + paint(cFaint, g) + ansi.TruncateLeft(s, col+1, "")
}
