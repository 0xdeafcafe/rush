package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	claudeagent "github.com/0xdeafcafe/rush/internal/adapters/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/memory"
)

func TestMemoryView(t *testing.T) {
	root := t.TempDir()
	// Agents run with TMPDIR under your home, so the walk up from cwd would
	// reach your own CLAUDE.md files; home is the test's.
	t.Setenv("HOME", root)
	cfg, cwd := filepath.Join(root, "cfg"), filepath.Join(root, "src", "app")
	proj := filepath.Join(cfg, "projects", claude.ProjectSlug(cwd))
	write := func(p, s string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(s), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	note := filepath.Join(proj, "memory", "work-on-main.md")
	write(filepath.Join(proj, "memory", "MEMORY.md"), "- [Work on main](work-on-main.md) — no branches\n- [Style](style.md) — commits\n")
	write(note, "---\nname: work-on-main\ndescription: no worktrees or branches\nmetadata:\n  type: feedback\n---\n\nWork on main.\n")
	write(filepath.Join(proj, "memory", "stray.md"), "---\nname: stray\ndescription: never indexed\n---\n")
	write(filepath.Join(root, "src", "CLAUDE.md"), "# parent\n")
	write(filepath.Join(cfg, "CLAUDE.md"), "@RTK.md\n\nAlso @missing.md, but not `@code.md`.\n")
	write(filepath.Join(cfg, "RTK.md"), strings.Repeat("x", 400))
	write(filepath.Join(cwd, ".claude", "settings.local.json"), "{}\n")
	write(filepath.Join(cwd, ".claude", "skills", "ship", "SKILL.md"), "---\nname: ship\ndescription: ships it\n---\n")
	write(filepath.Join(cfg, "commands", "git", "push.md"), "Push the branch\n")

	report := claudeagent.Adapter{}.Memory(agent.Profile{Kind: "claude", Dir: cfg}, cwd, "")
	files := report.Files
	var got []string
	for _, f := range files {
		got = append(got, f.Group+"|"+f.Name+"|"+f.Kind+"|"+map[bool]string{true: "missing"}[f.Missing]+"|"+f.Warn)
	}
	want := []string{
		"Auto memory|MEMORY.md|||",
		"Auto memory|work-on-main|feedback||",
		"Auto memory|stray|||not in MEMORY.md · found only if recall picks it",
		"Auto memory|style||missing|line 2 of MEMORY.md points here · no such note",
		"Instructions|" + tildify(filepath.Join(cfg, "CLAUDE.md")) + "|||",
		"Instructions|@RTK.md|||",
		"Instructions|@missing.md||missing|@missing.md doesn't lead to a file",
		"Instructions|CLAUDE.md||missing|",
		"Instructions|" + tildify(filepath.Join(root, "src", "CLAUDE.md")) + "|||",
		"Settings|" + tildify(filepath.Join(cfg, "settings.json")) + "||missing|",
		"Settings|.claude/settings.local.json|||",
		"Skills|ship|||",
		"Commands|/git:push|||",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("files:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	// What each costs: the imported file up front, a note on recall, a
	// skill's description up front and its body when used.
	for _, f := range files {
		switch f.Name {
		case "@RTK.md":
			if f.Up != 100 || f.Later != 0 {
				t.Errorf("import costs %d up front, %d later", f.Up, f.Later)
			}
		case "work-on-main":
			if f.Up != 0 || f.Later == 0 || f.When != "on recall" {
				t.Errorf("note: %+v", f)
			}
		case "ship":
			if f.Up == 0 || f.When != "when used" {
				t.Errorf("skill: %+v", f)
			}
		}
	}
	var kinds []string
	for _, p := range report.Problems {
		kinds = append(kinds, p.Kind)
	}
	if strings.Join(kinds, " ") != "broken-link broken-import orphan" {
		t.Errorf("problems: %v", kinds)
	}

	// The view draws them under their groups, the picked one open below;
	// why each is worth a look is judged as they're read.
	for i := range files {
		files[i].Review = memory.Why(files[i], time.Now())
	}
	m := &Model{snap: &fleet.Snapshot{}}
	c := &hostConn{kind: "claude", sess: convo.New(), open: map[string]bool{}, mem: files, memAt: time.Now()}
	c.memInfo = &report
	if !m.showView(c, "memory") || m.viewName(c) != "memory" {
		t.Fatal("memory has its own tab")
	}
	c.sel = "mem:" + note
	linked := false
	for _, l := range m.overviewSections(c, convo.Options{Width: 100, Now: time.Now()}) {
		linked = linked || l.Ref == "go:memory"
	}
	if !linked {
		t.Fatal("the Overview links to the Memory tab")
	}
	var out string
	for _, l := range m.memoryLines(c, convo.Options{Width: 100, Now: time.Now()}, 50) {
		out += ansi.Strip(l.Text) + "\n"
	}
	for _, w := range []string{"Auto memory", "How to work", "● no worktrees or branches", "No type", "never indexed  ! not in MEMORY.md", "1 to review · pick a note: r reviews", "Work on main.", "Skills", "◇ CLAUDE.md", "not written yet", "markdown",
		"tokens every session", "3 things to tidy", "MEMORY.md every session · a note when it's recalled", "never sent to Claude", "↳ @RTK.md", "≈100"} {
		if !strings.Contains(out, w) {
			t.Errorf("view missing %q:\n%s", w, out)
		}
	}

	// Enter hands the keys to the editor; esc hands them back.
	if _, used := m.memoryKey(c, tea.KeyPressMsg{}, "space"); !used || !c.memEdit || c.memEd.path != note {
		t.Fatal("space should edit the picked file")
	}
	if _, used := m.memoryKey(c, tea.KeyPressMsg{}, "esc"); !used || c.memEdit {
		t.Fatal("esc should leave the editor")
	}

	// Deleting a note asks first, then takes its line out of MEMORY.md.
	if _, used := m.memoryKey(c, tea.KeyPressMsg{}, "x"); !used || m.confirm == nil {
		t.Fatal("x should ask before deleting")
	}
	m.confirm.onYes()
	if exists(note) {
		t.Fatal("note not deleted")
	}
	b, _ := os.ReadFile(filepath.Join(proj, "memory", "MEMORY.md"))
	if strings.Contains(string(b), "work-on-main") || !strings.Contains(string(b), "style.md") {
		t.Fatalf("index after delete:\n%s", b)
	}
}

// typeIn types text into the editor a key at a time.
func typeIn(e *docEditor, text string) {
	for _, r := range text {
		s := string(r)
		switch r {
		case '\n':
			e.key(tea.KeyPressMsg{}, "enter")
			continue
		case ' ':
			s = "space"
		}
		e.key(tea.KeyPressMsg{Text: string(r), Code: r}, s)
	}
}

func TestDocEditorMarkdown(t *testing.T) {
	path := filepath.Join(t.TempDir(), "memory", "note.md")
	e := openDoc(path)
	e.view(60, 10, true)
	if !e.missing || e.kind != docMarkdown {
		t.Fatal("a new markdown file")
	}
	// A list goes on with enter, and an empty item ends it.
	typeIn(e, "# Notes\n- one\ntwo\n\n\nafter")
	if got := string(e.buf); got != "# Notes\n- one\n- two\n\nafter" {
		t.Fatalf("list: %q", got)
	}
	// Numbers count up; a task box carries on unticked; ctrl+t ticks it.
	e2 := openDoc(filepath.Join(t.TempDir(), "x.md"))
	typeIn(e2, "1. a\nb\n\n- [x] done\nnext")
	if got := string(e2.buf); got != "1. a\n2. b\n- [x] done\n- [ ] next" {
		t.Fatalf("numbers and boxes: %q", got)
	}
	e2.key(tea.KeyPressMsg{}, "ctrl+t")
	if !strings.HasSuffix(string(e2.buf), "- [x] next") {
		t.Fatalf("tick: %q", string(e2.buf))
	}
	// Undo takes back a word at a time.
	e2.key(tea.KeyPressMsg{}, "ctrl+z")
	e2.key(tea.KeyPressMsg{}, "ctrl+z")
	if strings.Contains(string(e2.buf), "next") {
		t.Fatalf("undo: %q", string(e2.buf))
	}

	// A memory note is checked for what it needs.
	if d := e.problems(); len(d) != 1 || !strings.Contains(d[0].msg, "no frontmatter") {
		t.Fatalf("memory note problems: %+v", d)
	}
	e.change([]rune("---\nname: n\ndescription: d\ntype: opinion\n---\n\nSee [x](missing.md).\n```go\nx := 1\n"), 0, false)
	var msgs []string
	for _, d := range e.problems() {
		msgs = append(msgs, fmt.Sprintf("%d:%s", d.line+1, d.msg))
	}
	got := strings.Join(msgs, "\n")
	for _, w := range []string{"7:missing.md doesn't exist", "8:this code block never closes", "4:type is one of"} {
		if !strings.Contains(got, w) {
			t.Errorf("problems missing %q:\n%s", w, got)
		}
	}

	// Saving writes it, making its folder.
	if err := e.save(); err != nil || e.dirty() {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(path); !strings.HasPrefix(string(b), "---\nname: n") {
		t.Fatalf("saved: %q", b)
	}
	// It draws: frontmatter, the cursor, the gutter's marks.
	rows := e.view(60, 12, true)
	if len(rows) != 12 || !strings.Contains(ansi.Strip(rows[1]), "2 name: n") {
		t.Fatalf("rows:\n%s", strings.Join(rows, "\n"))
	}
}

func TestDocEditorJSON(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := os.WriteFile(path, []byte("{\n\t\"a\": 1\n}\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	e := openDoc(path)
	e.view(60, 10, true)
	if e.indent != "\t" || len(e.problems()) != 0 {
		t.Fatalf("indent %q problems %+v", e.indent, e.problems())
	}
	// Brackets and quotes pair up, and enter inside a pair opens it.
	e.change([]rune(""), 0, false)
	typeIn(e, "{\"b")
	if got := string(e.buf); got != "{\"b\"}" {
		t.Fatalf("pairs: %q", got)
	}
	e.key(tea.KeyPressMsg{Text: "\"", Code: '"'}, "\"") // steps over the closing quote
	typeIn(e, ": [")
	e.key(tea.KeyPressMsg{}, "enter")
	if got := string(e.buf); got != "{\"b\": [\n\t\n]}" {
		t.Fatalf("enter in brackets: %q", got)
	}
	// A comma before the close is found, where it is.
	e.change([]rune("{\n\t\"a\": 1,\n}"), 0, false)
	d := e.problems()
	if len(d) != 1 || d[0].line != 1 || !strings.Contains(d[0].msg, "comma") {
		t.Fatalf("trailing comma: %+v", d)
	}
	e.change([]rune(`{"a": ["x",]}`), 0, false)
	if d := e.problems(); len(d) != 1 || !strings.Contains(d[0].msg, "comma") || d[0].col != 10 {
		t.Fatalf("trailing comma in a list: %+v", d)
	}
	e.change([]rune(`{"a":1,"a":2}`), 0, false)
	if d := e.problems(); len(d) != 1 || !strings.Contains(d[0].msg, `"a" is in this object twice`) {
		t.Fatalf("duplicate: %+v", d)
	}
	// ctrl+t lays it out; saving keeps the file's permissions.
	e.change([]rune(`{"b":[1,{}],"a":"<&>"}`), 0, false)
	e.key(tea.KeyPressMsg{}, "ctrl+t")
	out, err := formatJSON(string(e.buf), e.indent)
	if err != nil || out != "{\n\t\"b\": [\n\t\t1,\n\t\t{}\n\t],\n\t\"a\": \"<&>\"\n}\n" {
		t.Fatalf("format: %q %v", out, err)
	}
	if err := e.save(); err != nil {
		t.Fatal(err)
	}
	if st, _ := os.Stat(path); st.Mode().Perm() != 0o600 {
		t.Fatalf("mode %v", st.Mode())
	}
}
