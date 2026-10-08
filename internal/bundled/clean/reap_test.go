package clean

import (
	"slices"
	"testing"
	"time"
)

// A worktree goes only when nothing refuses it, and each refusal says why.
func TestRefusals(t *testing.T) {
	const day = 24 * time.Hour
	idleFor := 3 * day
	ok := reapFacts{Merged: "x is merged into origin/main", Idle: 4 * day}
	cases := []struct {
		name string
		edit func(*reapFacts)
		want []string
	}{
		{"clean, merged and idle", func(*reapFacts) {}, nil},
		{"upstream gone", func(f *reapFacts) { f.Merged = "x's upstream is gone" }, nil},
		{"detached in main", func(f *reapFacts) { f.Merged = "detached at a commit in origin/main" }, nil},
		{"main checkout", func(f *reapFacts) { f.Main, f.Changed = true, 3 }, []string{"the main checkout"}},
		{"check failed", func(f *reapFacts) { f.Err = "git status failed" }, []string{"git status failed"}},
		{"locked", func(f *reapFacts) { f.Locked = true }, []string{"locked"}},
		{"dirty", func(f *reapFacts) { f.Changed = 2 }, []string{"2 uncommitted files"}},
		{"process there", func(f *reapFacts) { f.Held = 1 }, []string{"1 process working there"}},
		{"processes unknown", func(f *reapFacts) { f.Held = -1 }, []string{"can't tell what runs there"}},
		{"unmerged", func(f *reapFacts) { f.Merged = "" }, []string{"its commits aren't all in the default branch"}},
		{"recent", func(f *reapFacts) { f.Idle = day }, []string{"touched 1d ago"}},
		{"dirty and unmerged", func(f *reapFacts) { f.Changed, f.Merged = 1, "" },
			[]string{"1 uncommitted file", "its commits aren't all in the default branch"}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := ok
			c.edit(&f)
			if got := refusals(f, idleFor); !slices.Equal(got, c.want) {
				t.Fatalf("refusals: %q, want %q", got, c.want)
			}
		})
	}
}
