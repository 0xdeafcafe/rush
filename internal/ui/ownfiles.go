package ui

import (
	"fmt"
	"path/filepath"
	"strings"

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
