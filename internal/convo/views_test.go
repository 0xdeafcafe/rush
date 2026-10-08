package convo

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
)

// JSON cut off mid-string, mid-array, or run on as JSON Lines, is laid out
// token by token rather than given up on.
func TestPrettyPartialJSON(t *testing.T) {
	got, lg, ok := prettyText(`{"a":{"b":[1,2,{}],"c":"x, y`)
	want := "{\n  \"a\": {\n    \"b\": [\n      1,\n      2,\n      {}\n    ],\n    \"c\": \"x, y"
	if !ok || lg != langJSON || got != want {
		t.Errorf("partial JSON:\n%s", got)
	}
	got, _, _ = prettyText("{\"a\":1}\n{\"b\":[]}")
	if got != "{\n  \"a\": 1\n}\n{\n  \"b\": []\n}" {
		t.Errorf("JSON lines:\n%s", got)
	}
	if _, _, ok := prettyText("[INFO] starting up"); ok {
		t.Error("a log line isn't JSON")
	}
}

// XML cut off mid-tag keeps what it has; an element of only text stays on
// one line, and HTML's void elements don't open a level.
func TestPrettyPartialXML(t *testing.T) {
	got, _, ok := prettyText(`<?xml version="1.0"?><root><a x="1>2">hi</a><b><c/><br><d>deep</d></b><e attr="cut`)
	want := strings.Join([]string{
		`<?xml version="1.0"?>`,
		`<root>`,
		`  <a x="1>2">hi</a>`,
		`  <b>`,
		`    <c/>`,
		`    <br>`,
		`    <d>deep</d>`,
		`  </b>`,
		`  <e attr="cut`,
	}, "\n")
	if !ok || got != want {
		t.Errorf("partial XML:\n%s", got)
	}
}

// Minified JSON opens pretty, prose opens as text, and bytes open in hex.
func TestBetterPretty(t *testing.T) {
	if !betterPretty(`{"a":1,"b":[1,2]}`) || betterPretty("{\"a\":1}\n{\"b\":2}\n{\"c\":3}") {
		t.Error("whole JSON is pretty; JSON Lines isn't")
	}
	if !binary("PNG\x00\x01") || binary("plain é text") {
		t.Error("binary is NULs or not UTF-8")
	}
}

// A hex row: the offset, bytes grouped in eights, the text beside them.
func TestHexRow(t *testing.T) {
	got := ansi.Strip(hexRow(0x10, []byte("ab\x00\t\xff"), -1, 16))
	if !strings.HasPrefix(got, "00000010  61 62 00 09 ff ") || !strings.HasSuffix(got, "│ab···│") {
		t.Errorf("row = %q", got)
	}
	bs := []byte("0123456789abcdefXYZ")
	if got := ansi.Strip(hexRow(0, bs[:8], -1, 8)); !strings.HasSuffix(got, "37  │01234567│") {
		t.Errorf("eight a row: %q", got)
	}
	if got := hexRow(0, []byte{0, ' ', 1, 0x80}, -1, 16); !strings.Contains(got, paint(cFaint, "00")) ||
		!strings.Contains(got, paint(cBlue, "20")) || !strings.Contains(got, paint(cDim, "01")) || !strings.Contains(got, paint(cOrange, "80")) {
		t.Errorf("each class its own colour: %q", got)
	}
}

// v cycles a command's minified JSON from pretty (as it opens) to text to
// hex and round; each view draws differently and says which it is.
func TestStepViewCycles(t *testing.T) {
	s := New()
	in, _ := jsonx.Marshal(map[string]string{"command": "curl -s api"})
	res, _ := jsonx.Marshal(map[string]string{"stdout": `{"ok":true,"items":[1,2]}`})
	st := &Step{ID: "b1", Tool: "Bash", Input: in, Result: res, Status: OK}
	s.byID["b1"] = st
	ref := "t1:s:b1"
	if v := s.NextView(ref, ""); v != ViewText {
		t.Fatalf("with the hex plugin off, pretty goes to text, not %q", v)
	}
	s.Hex = true
	var seen []string
	for v := ""; ; {
		v = s.NextView(ref, v)
		seen = append(seen, v)
		if v == ViewPretty || len(seen) > 4 {
			break
		}
	}
	if strings.Join(seen, " ") != "hex text pretty" {
		t.Fatalf("cycle = %v", seen)
	}
	d := &drawer{s: s, t: &Turn{}, ref: "t1", o: Options{Width: 120, Open: map[string]bool{ref: true}}, cw: 120}
	if v, _ := d.viewOf(st, ref); v != ViewPretty {
		t.Errorf("opens in %q", v)
	}
	open := func(v string) string {
		d := &drawer{s: s, t: &Turn{}, ref: "t1", o: Options{Width: 120, Selected: ref, Open: map[string]bool{ref: true}, View: map[string]string{ref: v}}, cw: 120}
		d.step(st, 0)
		var out []string
		for _, l := range d.lines {
			out = append(out, ansi.Strip(l.Text))
		}
		return strings.Join(out, "\n")
	}
	if p := open(ViewPretty); !strings.Contains(p, `"items": [`) || !strings.Contains(p, "pretty · v cycles") {
		t.Errorf("pretty:\n%s", p)
	}
	if p := open(ViewText); !strings.Contains(p, `{"ok":true,"items":[1,2]}`) || !strings.Contains(p, "text · v cycles") {
		t.Errorf("text:\n%s", p)
	}
	if p := open(ViewHex); !strings.Contains(p, "00000000  7b 22 6f 6b") || !strings.Contains(p, "hex · v cycles") {
		t.Errorf("hex:\n%s", p)
	}
	// Switching views changes the turn's cache key.
	if a, b := foldsByTurn(nil, map[string]string{ref: ViewHex}), foldsByTurn(nil, map[string]string{ref: ViewText}); a["t1"] == b["t1"] {
		t.Error("a view switch must redraw its turn")
	}
}
