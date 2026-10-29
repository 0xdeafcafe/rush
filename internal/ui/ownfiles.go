package ui

import (
	"fmt"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// ownChanges are the files the session in c edited outside its project,
// split: the memory it reads, which the Memory tab shows when it has one,
// and the rest (plans, scratch, notes), which the Overview shows.
func (c *hostConn) ownChanges() (mem, other []*convo.FileChange) {
	_, own := c.sess.ProjectChanges()
	memTab := canScreen(c, "memory")
	for _, fc := range own {
		if _, read := memFileAt(c.mem, fc.Path); memTab && (read || inMemoryDir(fc.Path)) {
			mem = append(mem, fc)
		} else {
			other = append(other, fc)
		}
	}
	return mem, other
}

// changeStat is the lines files add and delete.
func changeStat(files []*convo.FileChange) (add, del int) {
	for _, fc := range files {
		add, del = add+fc.Add, del+fc.Del
	}
	return add, del
}

// addDel is a +a −d pair, each side only when it's something.
func addDel(a, d int) string {
	var p []string
	if a > 0 {
		p = append(p, paint(cGreen, fmt.Sprintf("+%d", a)))
	}
	if d > 0 {
		p = append(p, paint(cRed, fmt.Sprintf("−%d", d)))
	}
	return strings.Join(p, " ")
}

// ownFileRows are a row for each file, its path short and its adds and
// deletes on the right, w wide.
func ownFileRows(files []*convo.FileChange, w int) []convo.Line {
	var out []convo.Line
	for _, fc := range files {
		stat := addDel(fc.Add, fc.Del)
		if fc.New {
			stat = paint(cGreen, fmt.Sprintf("new · %d lines", fc.Add))
		}
		name := paint(cText, filepath.Base(fc.Path)) + "  " + faint(tildify(filepath.Dir(fc.Path)))
		out = append(out, convo.Line{Text: fit(spread("    "+name, stat+"  ", w), w)})
	}
	return out
}

// setHelpers hands the session the conversations of its subagents read
// whole, and of the agents its shell ran, so its Changes has their edits.
func (c *hostConn) setHelpers() {
	if c.sess == nil {
		return
	}
	var hs []convo.Helper
	for _, sa := range c.subs {
		if st := c.sess.Step(sa.ToolUseID); st != nil && len(st.Children) > 0 {
			continue // its calls came in the session's own conversation
		}
		if t := c.fullSubs[sa.ID]; t != nil && t.Sess != nil {
			hs = append(hs, convo.Helper{Name: helperName(firstNonEmpty(sa.Description, sa.Type)), Sess: t.Sess})
		}
	}
	for _, id := range slices.Sorted(maps.Keys(c.spawns)) {
		r := c.spawns[id]
		if s := r.view().Sess; r.path != "" && s != nil && s != c.sess {
			hs = append(hs, convo.Helper{Name: helperName(firstNonEmpty(r.sp.Name, string(r.kind))), Sess: s})
		}
	}
	c.sess.Helpers = hs
}

// helperName is a subagent's name as a Changes row says it, short.
func helperName(s string) string { return cellw.Truncate(oneLine(s), 24, "…") }
