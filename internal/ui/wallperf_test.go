package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// benchWall is the Wall at w×h with n agents at work, each tile's preview
// read from a real transcript under ~/.claude/projects when there are any.
func benchWall(w, h, n int) *Model {
	m, _ := benchModel(w, h)
	// The tests run with a HOME of their own: $RUSH_BENCH_PROJECTS is
	// where real transcripts are, ~/.claude/projects say.
	paths, _ := filepath.Glob(filepath.Join(os.Getenv("RUSH_BENCH_PROJECTS"), "*", "*.jsonl"))
	sort.Slice(paths, func(i, j int) bool {
		a, _ := os.Stat(paths[i])
		c, _ := os.Stat(paths[j])
		return a.ModTime().After(c.ModTime())
	})
	agents := make([]*fleet.Agent, 0, n)
	for i := range n {
		a := &fleet.Agent{Key: fmt.Sprintf("default/w:%d", i), DisplayName: fmt.Sprintf("agent %d working on things", i), Acct: claude.DefaultAccount().Profile()}
		a.ID = fmt.Sprintf("%08x", i)
		a.Cwd, a.Repo, a.Branch, a.State, a.PID = "/work/rush", "/work/rush", "main", "working", 100+i
		a.UpdatedAt, a.CreatedAt = time.Now(), time.Now().Add(-time.Duration(i)*time.Minute)
		if i < len(paths) {
			a.TranscriptPath = paths[i]
			pv := claude.ReadPreview(paths[i], 128<<10)
			m.previews[a.Key] = previewEntry{p: pv}
		}
		agents = append(agents, a)
	}
	m.snap = &fleet.Snapshot{At: time.Now(), Agents: agents}
	m.rebuild()
	m.mode = modeWall
	m.View()
	return m
}

func BenchmarkWall(b *testing.B) {
	for _, n := range []int{6, 16, 30} {
		b.Run(strconv.Itoa(n), func(b *testing.B) {
			m := benchWall(250, 70, n)
			b.ReportAllocs()
			for b.Loop() {
				m.tick++
				m.View()
			}
		})
	}
}

// A tile's stream is drawn again when what it's drawn from changes, and
// only then.
func TestWallStreamKept(t *testing.T) {
	m := &Model{}
	p := claude.Preview{At: time.Unix(100, 0), Recent: []agent.Line{{Role: "assistant", Text: "looking at the wall"}}}
	a := m.wallStreamFor("k", p, true, "", 40, 5)
	if b := m.wallStreamFor("k", p, true, "", 40, 5); &a[0] != &b[0] {
		t.Fatal("drawn again with nothing changed")
	}
	drawn := func(w int) string { // wallStream's lines, each made w wide
		ls := wallStream(p, true, "", w, 5)
		for i := range ls {
			ls[i] = fit(ls[i], w)
		}
		return strings.Join(ls, "\n")
	}
	p.Recent = append(p.Recent, agent.Line{Role: "tool", Text: "Bash\x00go test ./..."})
	b := m.wallStreamFor("k", p, true, "", 40, 5)
	if strings.Join(b, "\n") != drawn(40) || len(b) == len(a) {
		t.Fatalf("a new message isn't drawn: %q", b)
	}
	if c := m.wallStreamFor("k", p, true, "", 20, 5); strings.Join(c, "\n") != drawn(20) {
		t.Fatalf("not drawn again for a new width: %q", c)
	}
}

// Every row of the Wall is exactly as wide as its body, tiles with long
// messages, short ones and none at all alike: rows aren't measured again.
func TestWallRowsFit(t *testing.T) {
	m := benchWall(163, 44, 7)
	long := strings.Repeat("a message long enough to wrap onto more rows than a tile has ", 6)
	for i, a := range m.snap.Agents {
		var recent []agent.Line
		for j := range i * 3 {
			recent = append(recent, agent.Line{Role: []string{"user", "assistant", "tool"}[j%3], Text: long[:(j*37)%len(long)] + "\x00" + "go test ./... -run 日本語"})
		}
		m.previews[a.Key] = previewEntry{p: claude.Preview{At: time.Unix(int64(i), 0), Recent: recent}}
	}
	for _, w := range []int{159, 120, 97} {
		for i, l := range m.wallBody(w, 40) {
			if got := cellw.String(l); got != w {
				t.Fatalf("w%d row %d is %d wide: %q", w, i, got, l)
			}
		}
	}
}
