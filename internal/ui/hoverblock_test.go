package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// hoverBlockModel is a pane whose last turn ran one command, opened, with
// a few lines of output under it and an answer after.
func hoverBlockModel(t *testing.T) (*Model, *hostConn) {
	m, _ := benchModel(200, 55)
	s := convo.New()
	s.Info.Cwd = "/work/rush"
	at := time.Now().Add(-time.Minute)
	apply := func(ev any) { s.Apply(ev, at); at = at.Add(time.Second) }
	apply(host.Sent{Text: "rebase onto main"})
	apply(headless.Message{Role: "assistant", ID: "m1", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash", Input: benchJSON(map[string]any{"command": "git rebase origin/main"})}}})
	out := "rebasing onto origin/main\nstopped at f8a5c56\nconflict internal/ui/mention_test.go\n1 file to resolve"
	apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: "b1", Text: out}}, ToolResult: benchJSON(map[string]any{"stdout": out, "stderr": ""})})
	apply(headless.Message{Role: "assistant", ID: "m2", Blocks: []headless.Block{{Type: "text", Text: "It stopped on a conflict."}}})
	apply(headless.Result{Subtype: "success"})
	c := m.host
	c.sess = s
	m.paneTop = m.topH()
	m.rushPane(120, 45)
	for i := c.bodyTop; i < len(c.rowRefs); i++ {
		if strings.Contains(c.rowRefs[i], ":s:b1") {
			c.open[c.rowRefs[i]] = true
		}
	}
	c.scrollOnly = false
	return m, c
}

func TestHoverLightsTheWholeBlock(t *testing.T) {
	m, c := hoverBlockModel(t)
	cold := m.rushPane(120, 45)
	head, block := -1, map[int]bool{}
	for i := c.bodyTop; i < len(c.rowRefs); i++ {
		if strings.Contains(c.rowRefs[i], ":s:b1") {
			head = i
		}
		if head >= 0 && strings.HasPrefix(c.ownerAt(i), c.rowRefs[head]) {
			block[i] = true
		}
	}
	if head < 0 || len(block) < 5 {
		t.Fatalf("want the step's row and its output, got row %d and %d rows", head, len(block))
	}
	for i := range block {
		if i != head && c.rowRefs[i] != "" {
			t.Fatalf("body row %d has a ref of its own: %q", i, c.rowRefs[i])
		}
	}
	lit := func(i int) {
		t.Helper()
		if !m.hoverPane(m.paneX()+4, m.paneTop+i) {
			t.Fatalf("row %d did not hover", i)
		}
		hot := m.rushPane(120, 45)
		for j := range cold {
			changed := hot[j] != cold[j]
			if ansi.Strip(hot[j]) != ansi.Strip(cold[j]) {
				t.Fatalf("hover changed row %d's text", j)
			}
			if changed != block[j] {
				t.Fatalf("hovering row %d: row %d lit %v, in the block %v: %q", i, j, changed, block[j], ansi.Strip(cold[j]))
			}
		}
		m.hoverPane(0, 0)
	}
	lit(head)
	for i := range block {
		if i != head {
			lit(i)
		}
	}
}
