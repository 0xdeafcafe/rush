package usage

import (
	"testing"
	"time"
)

func TestSwitchesAsItFills(t *testing.T) {
	now := time.Now()
	read := func(pct float64, ago time.Duration) Quota {
		// A week six days in: well under 1% an hour on average.
		return Quota{FetchedAt: now.Add(-ago), Windows: []Window{{ID: "seven_day", Percent: pct, Span: 7 * 24 * time.Hour, ResetsAt: now.Add(24 * time.Hour)}}}
	}
	// Slow: it runs to 99%.
	slow := Follow(read(96.9, 30*time.Minute), read(97, 0))
	if slow.NearlyOut("", Lead, now) {
		t.Fatalf("switched at 97%% filling %.0f%%/h", slow.Windows[0].Rate(now))
	}
	if got := slow.SwitchPoint("", Lead, now); got < 97 || got > 99 {
		t.Fatalf("slow switch point %.1f", got)
	}
	if !read(99, 0).NearlyOut("", Lead, now) {
		t.Fatal("99% isn't nearly out")
	}
	// Fast: 30% in 10 minutes, so 85% won't last the lead.
	fast := Follow(read(55, 10*time.Minute), read(85, 0))
	if !fast.NearlyOut("", Lead, now) {
		t.Fatalf("85%% filling %.0f%%/h isn't nearly out", fast.Windows[0].Rate(now))
	}
	// A reset since the last reading isn't a burn.
	if w := Follow(read(90, 10*time.Minute), read(2, 0)).Windows[0]; w.Burn != 0 {
		t.Fatalf("burn across a reset: %v", w.Burn)
	}
}

func TestAWeekAboutToResetIsntNearlyOut(t *testing.T) {
	now := time.Now()
	week := func(pct float64, ago time.Duration) Quota {
		return Quota{FetchedAt: now.Add(-ago), Windows: []Window{{ID: "seven_day", Percent: pct, Span: 7 * 24 * time.Hour, ResetsAt: now.Add(time.Hour)}}}
	}
	// One whole percent a minute apart is rounding, not 60% an hour.
	if q := Follow(week(91, time.Minute), week(92, 0)); q.NearlyOut("", Lead, now) {
		t.Fatalf("a 1%% step a minute apart switched: %.0f%%/h", q.Windows[0].Rate(now))
	}
	// Filling fast, but it resets before it would fill.
	five := Quota{FetchedAt: now, Windows: []Window{{ID: "five_hour", Percent: 85, Span: 5 * time.Hour, ResetsAt: now.Add(3 * time.Minute), Burn: 120}}}
	if five.NearlyOut("", Lead, now) {
		t.Fatal("85% resetting in 3 minutes is nearly out")
	}
	five.Windows[0].ResetsAt = now.Add(time.Hour)
	if !five.NearlyOut("", Lead, now) {
		t.Fatal("85% at 120%/h for the next hour isn't nearly out")
	}
}
