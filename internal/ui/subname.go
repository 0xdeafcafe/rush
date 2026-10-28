package ui

import (
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// Every place that lists a subagent run names it and marks how it stands
// the same way: what it was asked, then what runs it; ⠋ running, ✓ done,
// ✗ failed, ⏹ stopped.

// subTask is what run sa was asked, in a line.
func subTask(sa convo.Subagent) string {
	return firstNonEmpty(oneLineUI(sa.Description), sa.Type, "subagent")
}

// subTag is what runs sa: its model, and its harness's mark and name when
// that isn't the session's own.
func (c *hostConn) subTag(sa convo.Subagent) string {
	model := sa.Model
	if t := c.subTails[sa.ID]; t != nil {
		if s := t.Sess; len(s.Requests) > 0 && s.Requests[len(s.Requests)-1].Model != "" {
			model = s.Requests[len(s.Requests)-1].Model
		} else if s.Model != "" {
			model = s.Model
		}
	}
	tag := convo.RunTag(c.subKind(sa.ID), c.kindOf(), model, sa.Type)
	if tag == subTask(sa) {
		return ""
	}
	return tag
}

// subName is sa's name in at most w cells: its task in ink, the tag after
// it quieter. The task gives way first, down to half the room; then the
// tag does.
func (c *hostConn) subName(sa convo.Subagent, w int, ink string) string {
	task, tag := subTask(sa), c.subTag(sa)
	if tag == "" {
		return paint(ink, cellw.Truncate(task, w, "…"))
	}
	tw := min(cellw.String(tag), max(4, w/2-3))
	return paint(ink, cellw.Truncate(task, max(4, w-tw-3), "…")) + dim(" · ") + dim(cellw.Truncate(tag, tw, "…"))
}

// subMark is how a run stands, as a mark: its harness's spinner while it
// works, then ✓, ✗ or ⏹.
func subMark(k agent.Kind, status string, live bool, tick int) string {
	switch {
	case live:
		return spinOf(k, tick)
	case status == "failed":
		return paint(cRed, "✗")
	case status == "stopped":
		return dim("⏹")
	}
	return paint(cGreen, "✓")
}

// subWord is how a run stands, in a word.
func subWord(status string, live bool) string {
	switch {
	case live:
		return paint(cOrange, "running")
	case status == "failed":
		return paint(cRed, "failed")
	case status == "", status == "completed", status == "ended":
		return dim("done")
	}
	return dim(status)
}
