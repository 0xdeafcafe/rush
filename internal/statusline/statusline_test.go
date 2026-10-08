package statusline

import (
	"bytes"
	"os"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"

	_ "github.com/0xdeafcafe/rush/internal/adapters/claude"
)

func TestRenderRealInput(t *testing.T) {
	b, err := os.ReadFile("testdata/input.json")
	if err != nil {
		t.Fatal(err)
	}
	var in Input
	if err := jsonx.Unmarshal(b, &in); err != nil {
		t.Fatal(err)
	}
	in.Cost.USD, in.Context.Input = 1.5, 250_000
	l := Layout{Lines: [][]string{{"model", "effort", "folder", "repo", "context", "cost", "nope"}}, Sep: " | ", Plain: true}
	got := Render(in, l, "/nowhere", time.Now())
	want := "Opus 5.5 | medium | rush | 0xdeafcafe/rush | ctx 25% | $1.50"
	if got != want {
		t.Fatalf("got  %q\nwant %q", got, want)
	}
	// Segments with nothing to say drop out, separators and all.
	in.Cost.USD = 0
	if got := Render(in, Layout{Lines: [][]string{{"cost", "model"}}, Plain: true}, "", time.Now()); got != "Opus 5.5" {
		t.Fatalf("empty cost should vanish: %q", got)
	}
	// Coloured, it's the same text.
	col := Render(in, Layout{Lines: [][]string{{"model", "folder"}}}, "", time.Now())
	if regexp.MustCompile("\x1b\\[[0-9;]*m").ReplaceAllString(col, "") != "Opus 5.5 · rush" {
		t.Fatalf("coloured: %q", col)
	}
}

func TestRunAndOurs(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	var out bytes.Buffer
	if err := Run(strings.NewReader(`{"model":{"display_name":"Sonnet 5"},"cwd":"/x/y"}`), &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "Sonnet 5") || !strings.Contains(out.String(), "y") {
		t.Fatalf("run: %q", out.String())
	}
	for _, c := range []string{"rush statusline", "/Users/me/go/bin/rush statusline", "'/a b/rush' statusline"} {
		if !Ours(c) {
			t.Errorf("%q is rush's", c)
		}
	}
	if Ours("~/.claude/statusline.sh") {
		t.Error("a script of your own isn't rush's")
	}
}

func TestLinesAndYourOwn(t *testing.T) {
	var in Input
	in.Model.ID, in.Cwd = "claude-opus-5-5[1m]", "/src/rush"
	l := Layout{Lines: [][]string{{"model", "custom"}, {}, {"folder"}}, Plain: true, Custom: `cat >/dev/null; printf 'mine\nsecond'`}
	got := Render(in, l, "", time.Now())
	if got != "Opus 5.5 · mine\nsecond\nrush" {
		t.Fatalf("got %q", got)
	}
	for id, want := range map[string]string{"claude-sonnet-5": "Sonnet 5", "claude-haiku-4-5-20251001": "Haiku 4.5", "Opus 5.5 (1M context)": "Opus 5.5", "opus": "Opus"} {
		if got := ModelName(id); got != want {
			t.Errorf("ModelName(%q) = %q, want %q", id, got, want)
		}
	}
}
