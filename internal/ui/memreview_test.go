package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// run runs a Cmd and the sheet work it hands back, as the loop would.
func runSheet(m *Model, cmd func() any) {
	for cmd != nil {
		msg := cmd()
		sm, ok := msg.(sheetMsg)
		if !ok {
			return
		}
		next := sm.apply(m)
		if next == nil {
			return
		}
		cmd = func() any { return next() }
	}
}

func memFixture(t *testing.T) (agent.Profile, string, string) {
	t.Helper()
	root := t.TempDir()
	t.Setenv("HOME", root)
	t.Setenv("RUSH_HOME", filepath.Join(root, "rush"))
	cfg, cwd := filepath.Join(root, "cfg"), filepath.Join(root, "src", "app")
	os.MkdirAll(cwd, 0o755)
	dir := filepath.Join(cfg, "projects", claude.ProjectSlug(cwd), "memory")
	write := func(n claude.Note) string {
		p, err := claude.WriteNote(dir, n)
		if err != nil {
			t.Fatal(err)
		}
		return p
	}
	write(claude.Note{Name: "tests", Description: "run only the tests you touched in the repo", Type: "feedback", Body: "Targeted tests."})
	dup := write(claude.Note{Name: "tests-again", Description: "run only the tests you touched in the repository", Type: "feedback", Body: "Again."})
	old := write(claude.Note{Name: "launch", Description: "the launch is on Friday", Type: "project", Body: "Launch Friday."})
	past := time.Now().Add(-90 * 24 * time.Hour)
	os.Chtimes(old, past, past)
	_ = dup
	return agent.Profile{Kind: agent.LegacyKind, Dir: cfg}, cwd, dir
}

func TestMemReview(t *testing.T) {
	p, cwd, dir := memFixture(t)
	r, err := readMemReview(p, cwd, "app")
	if err != nil {
		t.Fatal(err)
	}
	var whys []string
	for _, it := range r.items {
		whys = append(whys, it.f.Name+": "+it.why)
	}
	got := strings.Join(whys, "\n")
	if len(r.items) != 2 || !strings.Contains(got, ": repeats tests") || !strings.Contains(got, "launch: unchanged since") {
		t.Fatalf("to review:\n%s", got)
	}
	m := &Model{snap: &fleet.Snapshot{}}
	s := &memReviewSheet{r: r, done: map[string]string{}}
	m.sheet = s
	body := ansi.Strip(strings.Join(s.body(m, 100, 30), "\n"))
	if !strings.Contains(body, "1 of 2") || !strings.Contains(body, "m merge into tests") || !strings.Contains(body, "g to CLAUDE.md") {
		t.Fatalf("sheet:\n%s", body)
	}
	// The repeat merges straight into the note it repeats.
	first := s.cur().f.Path
	if filepath.Base(first) == "launch.md" {
		runSheet(m, func() any { return s.key(m, tea.KeyPressMsg{}, "k")() })
		first = s.cur().f.Path
	}
	runSheet(m, func() any { return s.key(m, tea.KeyPressMsg{}, "m")() })
	if _, err := os.Stat(first); err == nil {
		t.Fatal("the repeat should be merged away")
	}
	kept := filepath.Join(dir, "tests.md")
	if filepath.Base(first) == "tests.md" {
		kept = filepath.Join(dir, "tests-again.md")
	}
	b, _ := os.ReadFile(kept)
	if !strings.Contains(string(b), "Targeted tests.") || !strings.Contains(string(b), "Again.") {
		t.Fatalf("merged note %q", b)
	}
	// Kept, a note isn't asked about again until it changes.
	if s.cur() != nil {
		runSheet(m, func() any { return s.key(m, tea.KeyPressMsg{}, "k")() })
	}
	if s.cur() != nil || !strings.Contains(ansi.Strip(strings.Join(s.body(m, 100, 30), "\n")), "1 kept") {
		t.Fatalf("review should be done: %+v", s.done)
	}
	if r, _ := readMemReview(p, cwd, "app"); len(r.items) != 0 {
		t.Fatalf("still to review: %+v", r.items)
	}
}

func TestRemember(t *testing.T) {
	p, cwd, dir := memFixture(t)
	old := rememberDraft
	defer func() { rememberDraft = old }()
	var asked string
	rememberDraft = func(what, around string) (agent.MemoryNote, error) {
		asked = what + "|" + around
		return agent.MemoryNote{Name: "no-mocks", Title: "No mocks", Description: "tests hit a real database", Type: "feedback", Body: "Use the real database."}, nil
	}
	a := &fleet.Agent{Key: "k", Acct: p, Cwd: cwd}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}}
	c := &hostConn{key: "k", kind: agent.LegacyKind, sess: convo.New(), open: map[string]bool{}}
	runSheet(m, func() any { return m.remember(c, "never mock the database", "")() })
	if asked != "never mock the database|" || m.confirm == nil || !strings.Contains(m.confirm.question, "No mocks") {
		t.Fatalf("asked %q, confirm %+v", asked, m.confirm)
	}
	runSheet(m, func() any { return m.confirm.onYes()() })
	if fm := claude.FrontMatter(filepath.Join(dir, "no-mocks.md")); fm["type"] != "feedback" {
		t.Fatalf("note not written: %v", fm)
	}
	ix, _ := os.ReadFile(filepath.Join(dir, "MEMORY.md"))
	if !strings.Contains(string(ix), "[No mocks](no-mocks.md) — tests hit a real database") {
		t.Fatalf("index %q", ix)
	}
}

func TestProjectMemoryRow(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	key := "/src/app"
	m.memCounts = map[string]*memSummary{key: {root: key, notes: 3, kinds: map[string]int{"feedback": 2, "project": 1}, review: 2, at: time.Now()}}
	var rows []workRow
	m.projectMemory(&project{key: key, repo: true}, 100, &rows)
	var out []string
	for _, r := range rows {
		out = append(out, ansi.Strip(r.line))
	}
	text := strings.Join(out, "\n")
	if !strings.Contains(text, "MEMORY  3 notes") || !strings.Contains(text, "2 feedback · 1 project") || !strings.Contains(text, "2 to review") {
		t.Fatalf("rows:\n%s", text)
	}
	if last := rows[len(rows)-1]; !last.pickable() || last.mem == nil {
		t.Fatal("the memory row is picked to review it")
	}
}

func TestMemReviewEdges(t *testing.T) {
	p, cwd, _ := memFixture(t)
	r, _ := readMemReview(p, cwd, "app")
	m := &Model{snap: &fleet.Snapshot{}}
	s := &memReviewSheet{r: r, done: map[string]string{}}
	// Merging with nothing else left to merge into says so, and never
	// picks past the list.
	s.r.notes = []agent.MemoryFile{s.cur().f}
	s.r.items[0].same = ""
	s.key(m, tea.KeyPressMsg{}, "m")
	if s.merging {
		t.Fatal("no picker with nothing to merge into")
	}
	s.merging = true
	s.key(m, tea.KeyPressMsg{}, "down")
	if cmd := s.key(m, tea.KeyPressMsg{}, "enter"); cmd != nil || s.pick != 0 {
		t.Fatalf("pick %d", s.pick)
	}
	s.merging = false
	// Back steps over notes the review already forgot.
	s.done[s.r.items[0].f.Path] = "forgotten"
	s.i = 1
	s.key(m, tea.KeyPressMsg{}, "left")
	if s.i != 1 {
		t.Fatalf("back landed on a forgotten note: %d", s.i)
	}
}

func TestRememberWaitsWhileYouType(t *testing.T) {
	p, cwd, dir := memFixture(t)
	old := rememberDraft
	defer func() { rememberDraft = old }()
	rememberDraft = func(string, string) (agent.MemoryNote, error) {
		return agent.MemoryNote{Name: "later", Description: "d", Type: "user", Body: "b"}, nil
	}
	a := &fleet.Agent{Key: "k", Acct: p, Cwd: cwd}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{a}}}
	c := &hostConn{key: "k", kind: agent.LegacyKind, sess: convo.New(), open: map[string]bool{}}
	cmd := m.remember(c, "x", "")
	c.input = []rune("my next message") // typed while it drafts
	runSheet(m, func() any { return cmd() })
	if m.confirm != nil || m.memDraft == nil {
		t.Fatal("the question shouldn't take the keys of what you're typing")
	}
	c.input = nil
	m.remember(c, "", "")
	if m.confirm == nil || !strings.Contains(m.confirm.question, "later") {
		t.Fatal("#remember alone shows the waiting note")
	}
	runSheet(m, func() any { return m.confirm.onYes()() })
	if _, err := os.Stat(filepath.Join(dir, "later.md")); err != nil {
		t.Fatal(err)
	}
}
