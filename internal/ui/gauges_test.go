package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/charmbracelet/x/ansi"
)

// The header's gauges: spend and account, the shorter limit first with a
// bar each, a warning when one fills before it resets, then the machine;
// none when they don't fit.
func TestHeaderGauges(t *testing.T) {
	m, _ := benchModel(200, 40)
	now := m.snap.At
	var a fleet.AccountView
	a.Current, a.Name = true, "borrowed"
	a.Quota = usage.Quota{FetchedAt: now, Windows: []usage.Window{
		{Label: "7d", Span: 168 * time.Hour, Percent: 71, ResetsAt: now.Add(50 * time.Hour)},
		{Label: "5h", Span: 5 * time.Hour, Percent: 38, ResetsAt: now.Add(3 * time.Hour)}}}
	m.snap.Accounts = append(m.snap.Accounts, a)
	g := m.headerGauges(1111, 100)
	if len(g) != 4 {
		t.Fatalf("%d rows", len(g))
	}
	rows := ansi.Strip(strings.Join(g, "\n"))
	if !strings.Contains(g[0], "borrowed") || !strings.HasPrefix(ansi.Strip(g[1]), "5h 38%") || !strings.HasPrefix(ansi.Strip(g[2]), "7d 71%") ||
		!strings.Contains(rows, "↻3h") || !strings.Contains(rows, "↻2d") || !strings.Contains(ansi.Strip(g[3]), "RAM") {
		t.Fatalf("gauges:\n%s", rows)
	}
	if m.headerGauges(1111, 20) != nil {
		t.Fatal("gauges drawn where they don't fit")
	}
}
