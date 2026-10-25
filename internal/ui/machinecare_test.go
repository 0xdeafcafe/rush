package ui

import (
	"strings"
	"testing"
	"time"
)

func TestCareReason(t *testing.T) {
	const g = 1 << 30
	for _, c := range []struct {
		name          string
		free          uint64
		diskGB        int
		pressed       time.Duration
		ramMin        int
		wantSomething bool
	}{
		{"plenty of disk, calm memory", 100 * g, 0, 0, 0, false},
		{"under the default 20G", 12 * g, 0, 0, 0, true},
		{"under 20G but set to 10G", 12 * g, 10, 0, 0, false},
		{"disk watch off", 1 * g, -1, 0, 0, false},
		{"pressed past the default 15m", 100 * g, 0, 16 * time.Minute, 0, true},
		{"pressed, not long enough", 100 * g, 0, 10 * time.Minute, 0, false},
		{"pressed 6m with 5m set", 100 * g, 0, 6 * time.Minute, 5, true},
		{"memory watch off", 100 * g, 0, time.Hour, -1, false},
	} {
		got := careReason(c.free, 500*g, c.diskGB, c.pressed, c.ramMin)
		if (got != "") != c.wantSomething {
			t.Errorf("%s: careReason = %q", c.name, got)
		}
	}
	if got := careReason(1, 0, 0, 0, 0); got != "" {
		t.Errorf("an unread disk is no reason: %q", got)
	}
}

func TestOffPick(t *testing.T) {
	for _, n := range []int{-1, 0, 10, 50} {
		if got := offValue(offPick(n, "%dG"), "%dG"); got != n {
			t.Errorf("offValue(offPick(%d)) = %d", n, got)
		}
	}
}

func TestCarePrompt(t *testing.T) {
	p := carePrompt("the disk has 9G free", []string{"Disk: 9G free of 500G."}, "")
	if !strings.Contains(p, "the disk has 9G free") || !strings.Contains(p, "Disk: 9G free of 500G.") {
		t.Fatalf("the reason and what rush knows are missing:\n%s", p)
	}
	if strings.Contains(p, "haven") {
		t.Fatalf("Haven's rules without Haven:\n%s", p)
	}
	if p := carePrompt("x", nil, "haven machine clean (its report; nothing was removed):\n* wt1"); !strings.Contains(p, "never drops databases") || !strings.Contains(p, "* wt1") {
		t.Fatalf("Haven's rules or report missing:\n%s", p)
	}
}

func TestCareSheetNotNowStartsNothing(t *testing.T) {
	m := &Model{}
	s := &careSheet{why: "the disk has 9G free", cur: 1}
	m.sheet = s
	if cmd := s.pick(m); cmd != nil || m.sheet != nil || len(m.care.agents) != 0 {
		t.Fatal("Not now must close the sheet and start nothing")
	}
}

func TestCareSheetBody(t *testing.T) {
	s := &careSheet{why: "the disk has 9G free, under the 20G you set", cur: 1}
	out := strings.Join(s.body(&Model{}, 100, 40), "\n")
	for _, want := range []string{"Start a cleanup agent", "Not now", "9G free"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
}
