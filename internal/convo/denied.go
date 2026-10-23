package convo

import (
	"strings"
)

// classifierRe is auto mode's refusal: why, before the instructions it
// gives Claude about what to do next.
var classifierRe = lazyRe(`(?s)denied by the Claude Code auto mode classifier\.\s*Reason:\s*(.*?)\.?\s*(?:If you have other tasks|IMPORTANT:|$)`)

// classified is why auto mode's classifier refused a step, and whether it
// did; the reason is its category (Credential Exploration) when it gave one.
func classified(st *Step) (string, bool) {
	if st.Status != Denied && st.Status != Failed {
		return "", false
	}
	m := classifierRe().FindStringSubmatch(toolErrTag.Replace(st.Output))
	if m == nil {
		return "", false
	}
	why := strings.TrimSpace(m[1])
	if strings.HasPrefix(why, "[") && strings.HasSuffix(why, "]") {
		why = strings.TrimSpace(why[1 : len(why)-1])
	}
	return why, true
}

// denial draws auto mode's refusal as a card under its step: what it
// called the action up top, and the rest of what it said inside, without
// the page of instructions it leaves for Claude.
func (d *drawer) denial(st *Step, indent int) {
	why, ok := classified(st)
	if !ok {
		return
	}
	room := min(d.cw, capRow) - indent - 4
	if room < 24 {
		return
	}
	room = min(room, 72)
	headL := paint(cYellow, "⊘") + " " + paint(cYellow+bold, "auto mode blocked this")
	var rows []string
	// A category fits on the edge; a sentence goes inside.
	category := why != "" && !strings.ContainsAny(why, ".,(") && len(why) <= 40
	switch {
	case category:
		headL += faint(" · ") + text(why)
	case strings.Contains(why, "transient"):
		rows = []string{dim("the classifier errored, and blocked to be safe")}
	case why != "" && why != "Blocked by classifier":
		rows = wrap(dim(why), room)
	}
	foot := dim("a permission rule allows it")
	if strings.Contains(why, "transient") {
		foot = dim("retrying often gets through")
	}
	if len(rows) == 0 {
		rows = []string{dim("Claude was told to find a safer way, or stop and ask you")}
	}
	d.box("", indent, headL, "", rows, foot, room, cWarnQ)
}
