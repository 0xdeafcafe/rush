package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A stopped task shows what it ran, laid out as a command, not the
// tool's JSON.
func TestTaskStopShowsCommand(t *testing.T) {
	s := New()
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 120, Verbose: true, Open: map[string]bool{}}, cw: 120}
	in, _ := jsonx.Marshal(map[string]string{"task_id": "b1"})
	cmd := "cat > run.sh <<'EOF'\n#!/bin/bash\necho hi\nEOF"
	res, _ := jsonx.Marshal(map[string]string{"message": "Successfully stopped task: b1 (" + cmd + ")", "task_id": "b1", "task_type": "local_bash", "command": cmd})
	st := &Step{ID: "k1", Tool: "TaskStop", Input: in, Result: res, Status: OK}
	d.body(st, 4)
	var all []string
	for _, l := range d.lines {
		all = append(all, stripANSI(l.Text))
	}
	j := strings.Join(all, "\n")
	if strings.Contains(j, "task_id") || !strings.Contains(j, "cat > run.sh") {
		t.Errorf("body:\n%s", j)
	}
	if lbl, _ := d.toolLabel(st, func(s string) string { return s }); strings.Contains(stripANSI(lbl), "echo hi") || !strings.Contains(stripANSI(lbl), "cat > run.sh <<'EOF' …") {
		t.Errorf("label = %q", stripANSI(lbl))
	}
}
