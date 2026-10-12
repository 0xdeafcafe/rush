package gate

import (
	"testing"
	"time"
)

func TestOrderIsFairByAgent(t *testing.T) {
	ts := []Ticket{
		{File: "ticket-1", Session: "busy", Running: true},
		{File: "ticket-2", Session: "busy", Running: true},
		{File: "ticket-3", Session: "busy"},
		{File: "ticket-4", Session: "calm", Running: true},
		{File: "ticket-5", Session: "calm"},
		{File: "ticket-6", Session: "new"},
		{File: "ticket-7", Session: "new"},
	}
	var got []string
	for _, t := range Order(ts) {
		got = append(got, t.File)
	}
	want := []string{"ticket-6", "ticket-7", "ticket-5", "ticket-3"}
	if len(got) != len(want) {
		t.Fatalf("order %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("order %v, want %v", got, want)
		}
	}
}

func TestAdmit(t *testing.T) {
	next := Ticket{Memory: 4 * gib}
	if !Admit(next, nil, 0, 0) {
		t.Fatal("held with nothing running")
	}
	// One running expects 4 GiB and holds 1 of it: 3 more are spoken for.
	ts := []Ticket{{Running: true, Memory: 4 * gib, State: State{Footprint: gib}}}
	if Admit(next, ts, 7*gib, gib) {
		t.Fatal("let in with 7 available, 1 kept free and 3 spoken for")
	}
	if !Admit(next, ts, 8*gib, gib) {
		t.Fatal("held with just room enough")
	}
	if r := Room(ts, gib, 2*gib); r != 0 {
		t.Fatalf("room %d when there's less than the reserve", r)
	}
}

func TestNiceAt(t *testing.T) {
	for _, c := range []struct {
		d    time.Duration
		want int
	}{{0, 0}, {29 * time.Second, 0}, {30 * time.Second, 5}, {119 * time.Second, 5}, {2 * time.Minute, 10},
		{9 * time.Minute, 10}, {10 * time.Minute, 15}, {3 * time.Hour, 15}} {
		if got := NiceAt(c.d); got != c.want {
			t.Errorf("NiceAt(%s) = %d, want %d", c.d, got, c.want)
		}
	}
}

func TestJudge(t *testing.T) {
	r := Rule{Memory: 2 * gib, Limit: 30 * time.Minute}
	ok := Look{Ran: time.Minute, Footprint: gib, HasAgent: true, AgentAlive: true, Pressure: PressureNormal,
		Newest: true, Others: 1, SincePause: time.Hour}
	for _, c := range []struct {
		name string
		look func(*Look)
		want Act
	}{
		{"healthy", func(*Look) {}, Carry},
		{"agent gone", func(l *Look) { l.AgentAlive = false }, Stop},
		{"orphaned, no session", func(l *Look) { l.HasAgent, l.Orphaned = false, true }, Stop},
		{"orphaned, session lives", func(l *Look) { l.HasAgent, l.Orphaned, l.SessionAlive = false, true, true }, Carry},
		{"orphaned under a live agent", func(l *Look) { l.Orphaned = true }, Carry},
		{"past its time", func(l *Look) { l.Ran = 31 * time.Minute }, Stop},
		{"past its cap", func(l *Look) { l.Footprint = 6*gib + 1 }, Stop},
		{"at its cap", func(l *Look) { l.Footprint = 6 * gib }, Carry},
		{"critical, newest", func(l *Look) { l.Pressure = PressureCritical }, Pause},
		{"critical, not newest", func(l *Look) { l.Pressure, l.Newest = PressureCritical, false }, Carry},
		{"critical, the last going", func(l *Look) { l.Pressure, l.Others = PressureCritical, 0 }, Carry},
		{"critical, one paused this tick", func(l *Look) { l.Pressure, l.SincePause = PressureCritical, time.Second }, Carry},
		{"warning, paused", func(l *Look) { l.Pressure, l.Paused = PressureWarn, true }, Carry},
		{"normal, paused", func(l *Look) { l.Paused = true }, Resume},
		{"critical, paused", func(l *Look) { l.Pressure, l.Paused = PressureCritical, true }, Carry},
	} {
		l := ok
		c.look(&l)
		if got, why := Judge(r, l); got != c.want {
			t.Errorf("%s: %v (%s), want %v", c.name, got, why, c.want)
		}
	}
}
