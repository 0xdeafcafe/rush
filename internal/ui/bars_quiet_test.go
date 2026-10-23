package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/state"
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

// queueQuota is an account's 5-hour and weekly windows, p5 and p7 used,
// resetting in r5 and r7.
func queueQuota(now time.Time, p5, p7 float64, r5, r7 time.Duration) usage.Quota {
	return usage.Quota{FetchedAt: now, Windows: []usage.Window{
		{ID: "five_hour", Label: "5h", Span: 5 * time.Hour, Percent: p5, ResetsAt: now.Add(r5)},
		{ID: "seven_day", Label: "7d", Span: 7 * 24 * time.Hour, Percent: p7, ResetsAt: now.Add(r7)},
	}}
}

// The header's accounts go in the order rush moves sessions through them,
// the one in use first with ▸, not the order they were added in.
func TestQueueIsTheMoveOnOrder(t *testing.T) {
	m, _ := accountsModel(t)
	now := m.snap.At
	m.snap.Logins = []fleet.LoginView{
		{Login: state.Login{ID: "w", Name: "work"}, Quota: queueQuota(now, 61, 30, 2*time.Hour, 5*24*time.Hour)},
		{Login: state.Login{ID: "p", Name: "personal"}, Current: true, Quota: queueQuota(now, 2, 4, 5*time.Hour, 7*24*time.Hour)},
		{Login: state.Login{ID: "s", Name: "side"}, Quota: queueQuota(now, 10, 20, 3*time.Hour, 6*time.Hour)},
	}
	m.rebuild()
	queues, single := m.acctQueues()
	var claude, zcodex []string
	for _, qu := range queues {
		for _, a := range qu.accts {
			switch qu.kind {
			case loginsKind:
				claude = append(claude, a.name)
			case "zcodex":
				zcodex = append(zcodex, a.name)
			}
		}
	}
	var chain []string
	for _, l := range fleet.Chain(m.snap.Logins, now) {
		chain = append(chain, l.Name)
	}
	// side's week ends in hours, so it's spent first.
	if strings.Join(claude, ",") != strings.Join(chain, ",") || strings.Join(claude, ",") != "personal,side,work" {
		t.Fatalf("Claude Code's queue %v, the order rush moves through %v", claude, chain)
	}
	if next, _ := fleet.NextLogin(m.snap.Logins, true); next.Name != claude[1] {
		t.Fatalf("rush moves to %s, the queue says %s", next.Name, claude[1])
	}
	if next, _, _ := nextAccount(accountsOf(m.accountRows(), "zcodex")); strings.Join(zcodex, ",") != "one,two" || next.name() != zcodex[1] {
		t.Fatalf("zcodex's queue %v, rush moves to %s", zcodex, next.name())
	}
	for _, r := range single {
		if r.kind == loginsKind || r.kind == "zcodex" {
			t.Fatalf("%s has accounts to move between, but is among those with one", r.kind)
		}
	}
	g := lookOf(loginsKind).glyph
	rows := planRows(queues, single, now, []int{200, 200, 200})
	if got := ansi.Strip(rows[0]); !strings.HasPrefix(got, g+" ▸ personal 2%/4%  → side 10%/20% ↻6h  → work 61%/30% ↻2h") {
		t.Fatalf("Claude Code's line: %q", got)
	}
	if got := ansi.Strip(rows[1]); !strings.Contains(got, "▸ one 99%") || !strings.Contains(got, "→ two 20%") {
		t.Fatalf("zcodex's line: %q", got)
	}
}

// An account that's out says so in red, and when it's back: in hours
// when that's within a day, else the day and time.
func TestQueueSaysWhenAnOutAccountIsBack(t *testing.T) {
	now := time.Date(2026, 10, 19, 12, 0, 0, 0, time.Local) // a Monday
	qu := acctQueue{kind: loginsKind, accts: []inQueue{
		{name: "personal", cur: true, q: queueQuota(now, 100, 40, 2*time.Hour, 5*24*time.Hour)},
		{name: "acme", q: queueQuota(now, 100, 100, 3*time.Hour, 69*time.Hour)},
	}}
	g := lookOf(loginsKind).glyph
	got := queueLine(qu, now, 200)
	if plain := ansi.Strip(got); plain != g+" ▸ personal out ↻2h  → acme out until Thu 09:00" {
		t.Fatal(plain)
	}
	if !strings.Contains(got, paint(cRed, "out")) {
		t.Fatal("out isn't red")
	}
}

// Short of room the header leaves out the others' resets, then their
// weekly figures, then the last of them, counted, but never the account
// in use.
func TestQueueFitsByDroppingInOrder(t *testing.T) {
	now := time.Now()
	qu := acctQueue{kind: loginsKind, accts: []inQueue{
		{name: "personal", cur: true, q: queueQuota(now, 70, 4, 2*time.Hour, 7*24*time.Hour)},
		{name: "work", q: queueQuota(now, 61, 30, 2*time.Hour, 5*24*time.Hour)},
		{name: "acme", q: queueQuota(now, 10, 20, 3*time.Hour, 6*24*time.Hour)},
	}}
	g := lookOf(loginsKind).glyph
	want := []string{
		g + " ▸ personal 70%/4% ↻2h  → work 61%/30% ↻2h  → acme 10%/20% ↻6d",
		g + " ▸ personal 70%/4% ↻2h  → work 61%/30%  → acme 10%/20%",
		g + " ▸ personal 70%/4% ↻2h  → work 61%  → acme 10%",
		g + " ▸ personal 70%/4% ↻2h  → work 61%  +1",
		g + " ▸ personal 70%/4% ↻2h  +2",
	}
	for _, w := range want {
		width := cellw.String(w)
		if got := ansi.Strip(queueLine(qu, now, width)); got != w {
			t.Fatalf("in %d cells: %q, want %q", width, got, w)
		}
		if got := ansi.Strip(queueLine(qu, now, width+1)); got != w {
			t.Fatalf("in %d cells: %q, want %q", width+1, got, w)
		}
	}
	// Narrower than the account in use: it's cut, not left out.
	if got := ansi.Strip(queueLine(qu, now, 12)); !strings.HasPrefix(got, g+" ▸ person") {
		t.Fatalf("in 12 cells: %q", got)
	}
	// The plan on one line keeps the other providers' glyphs while there's room.
	single := []acctRow{{kind: "codex", head: true, q: queueQuota(now, 100, 100, time.Hour, 4*24*time.Hour)}}
	if got := ansi.Strip(planRows([]acctQueue{qu}, single, now, []int{60})[0]); !strings.HasPrefix(got, g+" ▸ personal 70%/4% ↻2h  → work 61%") || !strings.HasSuffix(got, "  "+lookOf("codex").glyph+" 100%") || cellw.String(got) > 60 {
		t.Fatalf("one line: %q", got)
	}
}

// One account for each provider looks as it always has: no arrows, as
// there's nowhere to move to.
func TestQuietPlanWithOneAccountIsUnchanged(t *testing.T) {
	m, _ := accountsModel(t)
	m.snap.Logins = m.snap.Logins[:1]
	m.store.Config.SignIns = m.store.Config.SignIns[:1]
	m.rebuild()
	if queues, _ := m.acctQueues(); len(queues) != 0 {
		t.Fatalf("%d queues with one account each", len(queues))
	}
	got := ansi.Strip(m.quietPlan(200))
	if strings.ContainsAny(got, "▸→") || !strings.Contains(got, "5h 40%") {
		t.Fatal(got)
	}
}

// The plan alone on a line takes the rows the lines after it leave: a
// row for each provider with accounts to move between, and the rest go
// down. With no rows spare, it keeps to its one; a narrow header draws.
func TestHeaderGivesTheQueuesTheirRows(t *testing.T) {
	m, _ := accountsModel(t)
	m.dialog = nil
	m.w = 180
	m.bars.Top = statusline.Layout{Lines: [][]string{{"version"}, {"plan"}, {"memory"}}, Sep: " · "}
	rows := slicesMap(m.header(), ansi.Strip)
	if !strings.Contains(rows[0], "rush "+m.version) || !strings.Contains(rows[1], "▸ work") || !strings.Contains(rows[2], "▸ one") || !strings.Contains(rows[3], "G RAM") {
		t.Fatalf("rows:\n%s", strings.Join(rows, "\n"))
	}
	m.bars.Top = statusline.Layout{Lines: [][]string{{"version"}, {"plan"}, {"memory"}, {"today"}}, Sep: " · "}
	rows = slicesMap(m.header(), ansi.Strip)
	if !strings.Contains(rows[1], "▸ work") || strings.Contains(rows[1], "▸ one") || !strings.Contains(rows[2], "G RAM") {
		t.Fatalf("rows with none spare:\n%s", strings.Join(rows, "\n"))
	}
	m.w = 50
	for _, r := range m.header() {
		if cellw.String(r) > m.w {
			t.Fatalf("narrow row %q wider than %d", ansi.Strip(r), m.w)
		}
	}
}
