package ui

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

func TestProgramStatus(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	asks := &fleet.Agent{Key: "a", DisplayName: "fixer", PID: 1, Job: agent.Job{State: "blocked", Needs: "permission to run make"}}
	works := &fleet.Agent{Key: "b", DisplayName: "builder", PID: 2, Job: agent.Job{State: "working", Detail: "running go test\n"}}
	idle := &fleet.Agent{Key: "c", PID: 3, Job: agent.Job{State: "done"}}
	m.snap.Agents = []*fleet.Agent{asks, works, idle}

	out := rawOf(t, m)
	for _, want := range []string{
		"\x1b]7501;state=blocked:kind=question:msg=" + statusText("1 agent waiting on you", 2048) + ":app=rush:",
		"state=blocked:id=" + statusID("a") + ":kind=permission:title=" + statusText("fixer", 192),
		"state=working:id=" + statusID("b") + ":title=" + statusText("builder", 192) + ":msg=" + statusText("running go test", 2048),
	} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in %q", want, out)
		}
	}
	if strings.Contains(out, statusID("c")) {
		t.Errorf("an idle agent got a record: %q", out)
	}
	asks.Seen = true // seen but not answered: still waiting on you
	if c := m.programStatus(); c != nil {
		t.Errorf("a seen question changed its record: %v", c())
	}
	if c := m.programStatus(); c != nil {
		t.Errorf("nothing changed, yet it sent again")
	}

	asks.State = "done" // answered and finished: both records go, the root rests
	works.State = "done"
	out = rawOf(t, m)
	for _, want := range []string{
		"\x1b]7501;state=clear:id=" + statusID("a") + "\x1b\\",
		"\x1b]7501;state=clear:id=" + statusID("b") + "\x1b\\",
		"\x1b]7501;state=idle:app=rush:",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in %q", want, out)
		}
	}
}

func TestStatusTextDropsControls(t *testing.T) {
	if got, want := statusText("a\x1b]0;x\x07b", 192), statusText("a]0;xb", 192); got != want {
		t.Errorf("statusText kept control characters: %q", got)
	}
	if got := statusText(strings.Repeat("é", 200), 192); len(got) > 256 {
		t.Errorf("title over the limit: %d bytes", len(got))
	}
}

func rawOf(t *testing.T, m *Model) string {
	t.Helper()
	c := m.programStatus()
	if c == nil {
		t.Fatal("no report sent")
	}
	return fmt.Sprint(c())
}
