package ui

import (
	"os"
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

const permCmd = `mkdir -p "$TMPDIR/xterm-src" && cd "$TMPDIR/xterm-src" && for f in cursor.c screen.c; do curl -fsSL "https://example.com/$f" -o "$f" & done; wait; ls -la`

// A command to allow gets the framed card: its question and where it runs
// in the top edge, the chain a step to a line, and a button for each answer.
func TestPermissionCard(t *testing.T) {
	m, c := memAsk(t, "Bash", map[string]any{"command": permCmd})
	a := &fleet.Agent{DisplayName: "x", Cwd: "/src/app"}
	raw := strings.Join(m.cardRows(a, c, 90, 40), "\n")
	out := ansi.Strip(raw)
	for _, w := range []string{"╭─ ● Run this command?", "in /src/app", `$ mkdir -p "$TMPDIR/xterm-src" &&`,
		`  cd "$TMPDIR/xterm-src" &&`, "  wait;", "  ls -la", "y Allow once", "n Deny", "↑ to answer"} {
		if !strings.Contains(out, w) {
			t.Errorf("card missing %q:\n%s", w, out)
		}
	}
	if len(c.btns) < 2 {
		t.Fatalf("want its buttons to click, got %d", len(c.btns))
	}

	// With the keys, ←→ pick a button and enter presses it: left from
	// Allow once goes round to Deny.
	m.paneKey(tea.KeyPressMsg{}, "up")
	if !c.cardFocus {
		t.Fatal("up didn't give the card the keys")
	}
	m.paneKey(tea.KeyPressMsg{}, "left")
	bs := permBtns(c, c.sess.Pending()[0], 1<<30)
	if bs[c.btnPick][0] != "n" {
		t.Fatalf("left from Allow once picked %q, not Deny", bs[c.btnPick][2])
	}
	focused := strings.Join(m.cardRows(a, c, 90, 40), "\n")
	if !strings.Contains(ansi.Strip(focused), "←→ pick") {
		t.Fatalf("the card doesn't say ←→ pick:\n%s", ansi.Strip(focused))
	}
	// RUSH_PERMCARD_PREVIEW=file writes the card in colour, without the
	// keys and with them, to look at.
	if p := os.Getenv("RUSH_PERMCARD_PREVIEW"); p != "" {
		_ = os.WriteFile(p, []byte(raw+"\n\n"+focused+"\n"), 0o644)
	}
	if cmd := m.paneKey(tea.KeyPressMsg{}, "enter"); cmd == nil || c.cardFocus {
		t.Fatal("enter on Deny didn't answer")
	}
}

// A file change keeps its diff inside the frame.
func TestPermissionCardEdit(t *testing.T) {
	m, c := memAsk(t, "Edit", map[string]any{"file_path": "/src/app/main.go", "old_string": "a := 1", "new_string": "a := 2"})
	out := ansi.Strip(strings.Join(m.cardRows(&fleet.Agent{Cwd: "/src/app"}, c, 78, 40), "\n"))
	for _, w := range []string{"Change this file?", "main.go", "− a := 1", "+ a := 2"} {
		if !strings.Contains(out, w) {
			t.Errorf("card missing %q:\n%s", w, out)
		}
	}
}

func TestShellLines(t *testing.T) {
	for in, want := range map[string][]string{
		"a && b || c; d | e":             {"a &&", "b ||", "c;", "d |", "e"},
		`echo "x && y"; z`:               {`echo "x && y";`, "z"},
		"x=$(a && b) && c":               {"x=$(a && b) &&", "c"},
		"a 2>&1 | b":                     {"a 2>&1 |", "b"},
		"go test \\\n  ./... && echo ok": {"go test   ./... &&", "echo ok"},
		"one\ntwo":                       {"one", "two"},
	} {
		if got := shellLines(in); !slices.Equal(got, want) {
			t.Errorf("shellLines(%q) = %q, want %q", in, got, want)
		}
	}
}
