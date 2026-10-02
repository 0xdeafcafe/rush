package convo

import (
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// settled draws a turn you've gone past as its story: what you said, who
// answered, then what the agent said
// along the way, each batch of steps between as one row (red when one
// failed) and what the work made (pictures, drawings), and what it ended
// on. Opened, a batch draws as an open turn's steps do.
func (d *drawer) settled() {
	d.head()
	d.speaker()
	d.story()
	switch {
	case d.t.Stopped:
		d.add("", "", d.spine()+"   "+dim("⏹ stopped"), "")
	case d.t.Err != "":
		d.add("", "", d.spine()+blanks(gutter-1)+paint(cRed, "✗ "+d.t.Err), "")
	}
	if m := d.meta(); m != "" && d.t.Steps()+d.t.agentSteps() > 0 {
		d.add("", "", d.spine(), dim(m)+"  ")
	}
}

// storied is whether the open turn draws as its story: once it's done,
// at the default depth. While it runs it draws step by step.
func (d *drawer) storied() bool {
	return !d.t.Live && !waiting(d.t) && !d.o.Verbose && d.o.Depth == DepthDefault
}

// story is a finished turn's body: its words, its batches, the documents
// it wrote.
func (d *drawer) story() {
	items := d.t.Items
	var run []*Item
	at := 0
	flush := func() {
		if len(run) == 1 {
			d.item(run[0]) // one step is no batch
			run = nil
		}
		if len(run) > 0 {
			d.settledRun(d.ref+":run:"+strconv.Itoa(at), run)
			run = nil
		}
	}
	for i, it := range items {
		switch {
		case it.Kind == KStep && d.batched(it.Step):
			if len(run) == 0 {
				at = i
			}
			run = append(run, it)
			continue
		case it.Kind == KThinking:
			continue
		}
		flush()
		d.item(it)
		if it.Kind == KStep && d.doc(it.Step) {
			d.docPreview(it.Step)
		}
	}
	flush()
}

// speaker names the agent answering under what you said.
func (d *drawer) speaker() {
	if d.o.Agent == "" || len(d.t.Items) == 0 {
		return
	}
	d.add("", "", d.spine()+" "+faint("agent · ")+paint(cSub, d.o.Agent), "")
}

// batched is whether a finished turn's step folds into its batch: plain
// work, edits and failures do; what's part of the story stays out (a
// message, a subagent, a picture or drawing, an artifact). A failure
// folds too: its batch is red, and opened it shows why.
func (d *drawer) batched(st *Step) bool {
	switch glyphFor(st) {
	case "⇉", "◆", "◇":
		return false
	}
	switch st.Tool {
	case "ScheduleWakeup", "Monitor", "PushNotification", "SubagentHandback", "EnterPlanMode", "ExitPlanMode":
		return false // what it'll do next, or told you
	}
	return !hidden(st) && !messageTool(st.Tool) && !d.artifact(st) && !d.doc(st) && !d.replyOf(st)
}

// doc is a step that wrote a document, its last write in the turn: what
// it made, to read looking back, where its code edits are only counted.
func (d *drawer) doc(st *Step) bool {
	path, _ := d.written(st)
	switch strings.ToLower(filepath.Ext(path)) {
	case ".md", ".markdown", ".mdx", ".txt", ".rst", ".org", ".adoc":
	default:
		return false
	}
	for _, it := range slices.Backward(d.t.Items) {
		if it.Kind == KStep {
			if p, _ := d.written(it.Step); p == path {
				return it.Step == st
			}
		}
	}
	return false
}

// written is the file a step wrote whole and what it wrote: by the write
// tool, or a shell's cat into it from a heredoc.
func (d *drawer) written(st *Step) (path, content string) {
	switch {
	case st.Status != OK:
	case st.kind() == tool.Write:
		return d.rel(st.in().Path), st.in().Content
	case st.kind() == tool.Shell:
		if sh := d.shellShape(st.in().Command); sh.kind == "write" {
			return sh.what, sh.body
		}
	}
	return "", ""
}

// docPreview is the head of a document written, as its markdown reads.
func (d *drawer) docPreview(st *Step) {
	const most = 12
	_, content := d.written(st)
	if strings.TrimSpace(content) == "" || d.stepOpen(st, d.ref+":s:"+st.ID) {
		return // nothing to show, or shown whole already
	}
	lines := strings.Split(strings.TrimSpace(content), "\n")
	head := lines[:min(len(lines), most)]
	if strings.Count(strings.Join(head, "\n"), "```")%2 == 1 {
		head = append(slices.Clone(head), "```") // a fence cut off closes
	}
	d.markdown(strings.Join(head, "\n"), gutter+2, cSub, false)
	if n := len(lines) - len(head); n > 0 {
		d.add("", "", d.spine()+blanks(gutter+1)+faint("… "+plural(n, "more line")), "")
	}
	d.gap()
}

// artifact is a step whose result is the point of looking back: a picture
// it read or was given, or a drawing it showed.
func (d *drawer) artifact(st *Step) bool {
	return len(st.Images) > 0 || st.kind() == tool.Read && thumbable(d.abs(st.in().Path)) || strings.HasSuffix(st.Tool, "__show")
}

// settledRun is a run of steps in a turn gone past: one row saying how
// many, the files they changed and by how much, and any that failed, then
// the cards for what they made. Opened, every step as an open turn has it.
func (d *drawer) settledRun(ref string, run []*Item) {
	if d.o.Open[ref] {
		d.railed(gutter-2, true, func() {
			d.add(ref, "", d.spine()+blanks(gutter-1)+faint("▾ hide "+plural(len(run), "step")), "")
			for _, x := range run {
				d.step(x.Step, 0)
			}
		})
		return
	}
	files := map[string]bool{}
	add, del, failed := 0, 0, 0
	for _, x := range run {
		st := x.Step
		if st.Status == Failed || d.testsFailed(st) {
			failed++
		}
		if k := st.kind(); st.Status != OK || k != tool.Edit && k != tool.Write {
			continue
		}
		// Counted as the changes view counts them: from what it did.
		files[st.in().Path] = true
		if o := st.out(); o.Created {
			add += countLines(st.in().Content)
		} else {
			for _, p := range o.Patches {
				for _, l := range p.Lines {
					switch {
					case strings.HasPrefix(l, "+"):
						add++
					case strings.HasPrefix(l, "-"):
						del++
					}
				}
			}
		}
	}
	// What failed is red in it; the rest stays quiet.
	var names []string
	for _, g := range d.runGroups(run) {
		if g.failed {
			names = append(names, paint(cRed, g.name))
		} else {
			names = append(names, faint(g.name))
		}
	}
	left := d.spine() + blanks(gutter-1) + faint("▸ "+plural(len(run), "step")) + "  " + strings.Join(names, faint(", "))
	if len(files) > 0 {
		left += "  " + paint(cGreen, "+"+strconv.Itoa(add))
		if del > 0 {
			left += " " + paint(cRed, "−"+strconv.Itoa(del))
		}
		left += faint(" in " + plural(len(files), "file"))
	}
	if failed > 0 {
		left += "  " + paint(cRed, "✗ "+strconv.Itoa(failed)+" failed")
	}
	d.worked = true
	d.add(ref, "", left, "")
	for _, x := range run {
		for _, c := range d.stepCards(x.Step) {
			if !d.superseded(x.Step, c.kind) {
				d.card(c, gutter+2)
			}
		}
	}
}

// superseded is a tests or build card a later run in the turn tells again,
// passed or not:
// looking back, only how the last one came out matters.
func (d *drawer) superseded(st *Step, kind string) bool {
	if kind != "tests" && kind != "build" {
		return false
	}
	after := false
	for _, it := range d.t.Items {
		if it.Kind != KStep {
			continue
		}
		if it.Step == st {
			after = true
			continue
		}
		if !after || it.Step.kind() != tool.Shell {
			continue
		}
		cmd := blankHeredocs(it.Step.in().Command)
		if kind == "tests" && testVerb.MatchString(cmd) || kind == "build" && buildVerb.MatchString(cmd) {
			return true
		}
	}
	return false
}
