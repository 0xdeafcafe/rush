package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/statusline"
	"github.com/0xdeafcafe/rush/internal/sysinfo"
	"github.com/charmbracelet/x/ansi"
)

func TestPlanSummaryHasStableWindowsAndWidth(t *testing.T) {
	now := time.Now()
	q := usage.Quota{FetchedAt: now, Windows: []usage.Window{
		{ID: "weekly", Label: "7d", Span: 7 * 24 * time.Hour, Percent: 35, ResetsAt: now.Add(4 * 24 * time.Hour)},
		{ID: "short", Label: "5h", Span: 5 * time.Hour, Percent: 58, ResetsAt: now.Add(3 * time.Hour)},
	}}
	if got := ansi.Strip(planSummary("Anthropic", q, now)); got != "Anthropic 5h 58% ↻3h  7d 35% ↻4d" {
		t.Fatal(got)
	}
	// The short window first, whatever's used.
	q.Windows[0].Percent, q.Windows[1].Percent = 99, 9
	got := ansi.Strip(planSummary("Anthropic", q, now))
	if strings.Index(got, "5h") > strings.Index(got, "7d") || !strings.Contains(got, "7d 99%") {
		t.Fatal(got)
	}
}

func TestPlanSummaryWarnsWhenAWindowRunsOutBeforeItResets(t *testing.T) {
	now := time.Now()
	q := usage.Quota{FetchedAt: now, Windows: []usage.Window{
		{ID: "short", Label: "5h", Span: 5 * time.Hour, Percent: 86, Burn: 28, ResetsAt: now.Add(3 * time.Hour)},
	}}
	if got := ansi.Strip(planSummary("Anthropic", q, now)); got != "Anthropic 5h 86% out in 20m ↻3h" {
		t.Fatal(got)
	}
}

func TestPlanSummaryMarksStaleAndExpiredQuota(t *testing.T) {
	now := time.Now()
	q := usage.Quota{FetchedAt: now.Add(-time.Hour), Windows: []usage.Window{{Label: "5h", Percent: 99, ResetsAt: now.Add(-time.Minute)}}}
	// Even a recent fetch cannot prove new allowance once its reset elapsed.
	q.FetchedAt = now
	got := ansi.Strip(planSummary("OpenAI", q, now))
	if !strings.Contains(got, "5h 99%") || !strings.Contains(got, "refresh needed") {
		t.Fatal(got)
	}
	q.Windows[0].ResetsAt = now.Add(time.Hour)
	q.FetchedAt = now.Add(-time.Hour)
	if got := ansi.Strip(planSummary("OpenAI", q, now)); !strings.Contains(got, "5h 99%") || !strings.Contains(got, "stale") {
		t.Fatal(got)
	}
}

func TestQuietSystemOnlyShowsActionableConditions(t *testing.T) {
	goodBat := sysinfo.Battery{Present: true, Percent: 80}
	goodDisk := sysinfo.Disk{Total: 1 << 40, Free: 200 << 30}
	if got := systemAlerts(goodBat, goodDisk, true, true, true); got != "" {
		t.Fatalf("healthy metrics crowd bar: %s", got)
	}
	badBat := sysinfo.Battery{Present: true, Percent: 10}
	badDisk := sysinfo.Disk{Total: 1 << 40, Free: 3 << 30}
	got := ansi.Strip(systemAlerts(badBat, badDisk, true, false, false))
	if got != "network offline · disk low · battery low" {
		t.Fatal(got)
	}
	badBat.Charging = true
	if strings.Contains(systemAlerts(badBat, goodDisk, false, false, false), "battery") {
		t.Fatal("charging battery flagged")
	}
}

func TestDefaultTopIsQuietButDetailedSegmentsRemainAvailable(t *testing.T) {
	top := statusline.DefaultTop()
	if len(top.Lines) != 2 || len(top.Lines[1]) != 0 {
		t.Fatal("default top should occupy one line")
	}
	for _, id := range []string{"usage", "accounts", "cpu", "tokens", "tmp", "battery", "disk", "net"} {
		if top.Shown(id) {
			t.Errorf("noisy segment %s remains in default", id)
		}
		if _, ok := findBarSeg(barTop, id); !ok {
			t.Errorf("custom segment %s removed", id)
		}
	}
	for _, id := range []string{"today", "plan", "memory", "system"} {
		if !top.Shown(id) {
			t.Errorf("missing %s", id)
		}
	}
}
