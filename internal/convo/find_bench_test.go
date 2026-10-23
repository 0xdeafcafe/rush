package convo

import (
	"bytes"
	"os"
	"strings"
	"testing"
)

// BenchmarkHoldsAll is the pass over raw bytes every search makes of every
// transcript, over RUSH_SEARCH_FILES (a file naming transcripts, a line
// each), read into memory first: a word that's there, one that isn't
// (read to the end, typos too), and one too short for typos.
func BenchmarkHoldsAll(b *testing.B) {
	list := os.Getenv("RUSH_SEARCH_FILES")
	if list == "" {
		b.Skip("RUSH_SEARCH_FILES names the transcripts")
	}
	names, err := os.ReadFile(list)
	if err != nil {
		b.Fatal(err)
	}
	var data [][]byte
	total := 0
	for _, p := range strings.Fields(string(names)) {
		d, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		data, total = append(data, d), total+len(d)
	}
	for _, q := range []string{"worktree", "zebracorn", "fix"} {
		b.Run(q, func(b *testing.B) {
			b.SetBytes(int64(total))
			for b.Loop() {
				for _, d := range data {
					_, _ = holdsAll(bytes.NewReader(d), []string{q})
				}
			}
		})
	}
}
