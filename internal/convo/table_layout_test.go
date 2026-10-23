package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
)

// A table keeps every cell at any width: framed where its columns fit,
// each row's cells one under the other where they don't.
func TestTableKeepsAllCellsAtAnyWidth(t *testing.T) {
	s := New()
	markdown := "| A | B | C | D | E | F |\n|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 |"
	for _, width := range []int{14, 32, 80} {
		lines := s.Answer(markdown, width)
		out := plain(lines)
		for _, value := range []string{"A", "B", "C", "D", "E", "F", "1", "2", "3", "4", "5", "6"} {
			if !strings.Contains(out, value) {
				t.Fatalf("width %d lost %s: %s", width, value, out)
			}
		}
		for _, line := range lines {
			if cellw.String(line.Text) > width {
				t.Fatalf("width %d overflow:\n%s", width, out)
			}
		}
		if framed := strings.Contains(out, "╭"); framed != (width >= 32) {
			t.Fatalf("width %d framed=%v:\n%s", width, framed, out)
		}
	}
}

// A framed table: header over a double rule, a rule between rows once one
// wraps, words wrapped whole, inline code coloured rather than ticked.
func TestTableFrame(t *testing.T) {
	s := New()
	md := "| Module | What it does | State |\n|---|---|--:|\n| entitlement | handles usage and the limits each plan sets for a project | `ready` |\n| billing | sends invoices | 12 |"
	lines := s.Answer(md, 60)
	got := strings.Split(strings.TrimRight(plain(lines), "\n"), "\n")
	want := []string{
		"  ╭─────────────┬─────────────────────────────────┬───────╮",
		"  │ Module      │ What it does                    │ State │",
		"  ╞═════════════╪═════════════════════════════════╪═══════╡",
		"  │ entitlement │ handles usage and the limits    │ ready │",
		"  │             │ each plan sets for a project    │       │",
		"  ├─────────────┼─────────────────────────────────┼───────┤",
		"  │ billing     │ sends invoices                  │    12 │",
		"  ╰─────────────┴─────────────────────────────────┴───────╯",
	}
	// The answer stands a row clear of what came before.
	for len(got) > 0 && strings.TrimSpace(got[0]) == "" {
		got = got[1:]
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("table\n got:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	all := ""
	for _, l := range lines {
		all += l.Text
	}
	if strings.Contains(all, "`") || !strings.Contains(all, cWhite+"ready") {
		t.Errorf("inline code should be coloured, not ticked: %q", all)
	}
}

// Rows that each fit a line need no rules between them.
func TestTableOneLineRowsHaveNoInnerRules(t *testing.T) {
	out := plain(New().Answer("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |", 60))
	if strings.Contains(out, "├") || !strings.Contains(out, "╞") || !strings.Contains(out, "╰") {
		t.Fatalf("one-line rows:\n%s", out)
	}
}

// Too narrow for a frame, each row lists its cells under the header's names.
func TestTableRecordsWhenNarrow(t *testing.T) {
	md := "| Module | What it does | State | Notes |\n|---|---|---|---|\n| entitlement | handles usage | ready | the replay runs after the migration |\n| billing | sends invoices | blocked | none |"
	got := plain(New().Answer(md, 40))
	for _, want := range []string{"Module        entitlement", "What it does  handles usage", "Notes         the replay runs after", "              the migration", "─────", "Module        billing"} {
		if !strings.Contains(got, want) {
			t.Fatalf("missing %q:\n%s", want, got)
		}
	}
	if strings.Contains(got, "╭") {
		t.Fatalf("framed at 40:\n%s", got)
	}
}
