package remind

import (
	"errors"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestParse(t *testing.T) {
	now := time.Date(2026, 10, 28, 14, 0, 0, 0, time.Local)
	for in, want := range map[string]time.Time{
		"25m":                  now.Add(25 * time.Minute),
		"in 2h":                now.Add(2 * time.Hour),
		"1h30m":                now.Add(90 * time.Minute),
		"15:30":                time.Date(2026, 10, 28, 15, 30, 0, 0, time.Local),
		"9:00":                 time.Date(2026, 10, 29, 9, 0, 0, 0, time.Local), // passed today
		"3pm":                  time.Date(2026, 10, 28, 15, 0, 0, 0, time.Local),
		"3:30pm":               time.Date(2026, 10, 28, 15, 30, 0, 0, time.Local),
		"tomorrow 9:00":        time.Date(2026, 10, 29, 9, 0, 0, 0, time.Local),
		"Tomorrow at 3pm":      time.Date(2026, 10, 29, 15, 0, 0, 0, time.Local),
		"2026-11-02 09:00":     time.Date(2026, 11, 2, 9, 0, 0, 0, time.Local),
		"2026-11-02T09:00":     time.Date(2026, 11, 2, 9, 0, 0, 0, time.Local),
		"2026-10-28T15:00:00Z": time.Date(2026, 10, 28, 15, 0, 0, 0, time.UTC),
	} {
		got, err := Parse(in, now)
		if err != nil || !got.Equal(want) {
			t.Errorf("Parse(%q) = %v, %v; want %v", in, got, err, want)
		}
	}
	for _, in := range []string{"", "soon", "-5m", "2026-10-27 09:00"} {
		if _, err := Parse(in, now); err == nil {
			t.Errorf("Parse(%q) = no error", in)
		}
	}
}

// save writes a reminder without starting a waiter, as Add would.
func save(t *testing.T, session string, due, now time.Time) Reminder {
	t.Helper()
	r, err := Add(session, "look at the deploy", due, now)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func TestWaiterSendsWhenDueAndDrops(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Date(2026, 10, 28, 14, 0, 0, 0, time.UTC)
	starting = func(string) error { return nil }
	t.Cleanup(func() { starting = startWaiter })
	r := save(t, "s1", now.Add(150*time.Second), now)
	var slept []time.Duration
	var sent []string
	w := Waiter{
		Now:   func() time.Time { return now },
		Sleep: func(d time.Duration) { slept = append(slept, d); now = now.Add(d) },
		Deliver: func(got Reminder, text string) error {
			sent = append(sent, got.Session+": "+text)
			return nil
		},
		Gone: func(string) bool { return false },
	}
	if err := w.Run(r.ID); err != nil {
		t.Fatal(err)
	}
	if want := []time.Duration{time.Minute, time.Minute, 30 * time.Second}; !equal(slept, want) {
		t.Errorf("slept %v; want %v", slept, want)
	}
	if len(sent) != 1 || !strings.Contains(sent[0], "s1: <rush-reminder") || !strings.Contains(sent[0], "look at the deploy") {
		t.Errorf("sent %q", sent)
	}
	if len(List("s1")) != 0 {
		t.Error("a sent reminder is kept")
	}
	if left, _ := filepath.Glob(filepath.Join(Dir(), r.ID+".*")); len(left) != 0 {
		t.Errorf("left %v", left)
	}
}

func TestWaiterEndsWhenCancelled(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now()
	starting = func(string) error { return nil }
	t.Cleanup(func() { starting = startWaiter })
	r := save(t, "s1", now.Add(time.Hour), now)
	other := save(t, "s2", now.Add(time.Hour), now)
	if err := Cancel("s2", r.ID); err == nil {
		t.Error("one session cancelled another's reminder")
	}
	w := Waiter{
		Now:     func() time.Time { return now },
		Sleep:   func(time.Duration) { _ = Cancel("s1", r.ID) },
		Deliver: func(Reminder, string) error { t.Error("a cancelled reminder was sent"); return nil },
		Gone:    func(string) bool { return false },
	}
	if err := w.Run(r.ID); err != nil {
		t.Fatal(err)
	}
	if got := List(""); len(got) != 1 || got[0].ID != other.ID {
		t.Errorf("left %v; want only %s", got, other.ID)
	}
}

func TestWaiterGivesUpAfterTries(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now()
	starting = func(string) error { return nil }
	t.Cleanup(func() { starting = startWaiter })
	r := save(t, "s1", now.Add(time.Second), now)
	n := 0
	w := Waiter{
		Now:     func() time.Time { return now },
		Sleep:   func(d time.Duration) { now = now.Add(d) },
		Deliver: func(Reminder, string) error { n++; return errors.New("host down") },
		Gone:    func(string) bool { return false },
	}
	if err := w.Run(r.ID); err == nil {
		t.Error("no error after every try failed")
	}
	if n != tries {
		t.Errorf("tried %d times; want %d", n, tries)
	}
}

func TestWaiterDropsAGoneSessions(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now()
	starting = func(string) error { return nil }
	t.Cleanup(func() { starting = startWaiter })
	r := save(t, "s1", now.Add(time.Hour), now)
	w := Waiter{
		Now:     func() time.Time { return now },
		Sleep:   func(time.Duration) { t.Error("slept for a gone session") },
		Deliver: func(Reminder, string) error { t.Error("sent to a gone session"); return nil },
		Gone:    func(string) bool { return true },
	}
	if err := w.Run(r.ID); err != nil || len(List("")) != 0 {
		t.Errorf("Run = %v, left %v", err, List(""))
	}
}

func TestMessageSaysWhenLate(t *testing.T) {
	due := time.Date(2026, 10, 28, 14, 0, 0, 0, time.UTC)
	r := Reminder{ID: "ab", Text: "x", Due: due, Created: due.Add(-time.Hour)}
	if strings.Contains(Message(r, due.Add(time.Minute)), "late") {
		t.Error("a minute is called late")
	}
	if !strings.Contains(Message(r, due.Add(time.Hour)), "1h late") {
		t.Error(Message(r, due.Add(time.Hour)))
	}
}

func equal(a, b []time.Duration) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
