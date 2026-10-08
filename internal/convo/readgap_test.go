package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A Markdown file's rows with source pasted into them don't keep the
// source's indentation as a gap in the middle of the line.
func TestReadMarkdownGaps(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	d := &drawer{s: s, t: &Turn{}, o: Options{Width: 200, Verbose: true, Open: map[string]bool{}}, cw: 200}
	in, _ := jsonx.Marshal(map[string]string{"file_path": "/work/smells.md"})
	res := "     1\t# Sweep\n     2\t201\t0725ca30c7\tmodules/a.trpc.ts:48\t            \"the key is loaded\"\n     3\t| a   | b |\n"
	d.body(&Step{Tool: "Read", Input: in, Output: res, Status: OK}, 4)
	var all []string
	for _, l := range d.lines {
		all = append(all, strings.TrimRight(stripANSI(l.Text), " "))
	}
	j := strings.Join(all, "\n")
	if !strings.Contains(j, "modules/a.trpc.ts:48  \"the key") || !strings.Contains(j, "| a   | b |") {
		t.Errorf("read:\n%s", j)
	}
}

func TestShortAbs(t *testing.T) {
	p := "/opt/agtop/claude/a4bb3b77-4f15/projects/-Users-lw-Source/memory/smells.md"
	if got := shortAbs(p); got != "/opt/…/memory/smells.md" {
		t.Errorf("shortAbs = %q", got)
	}
	if got := shortAbs("/opt/a/b.md"); got != "/opt/a/b.md" {
		t.Errorf("short path changed: %q", got)
	}
}
