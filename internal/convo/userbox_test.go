package convo

import (
	"fmt"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

func fillOf(ls []Line) (filled int, others int) {
	for _, l := range ls {
		if strings.Contains(l.Text, bgUser) {
			filled++
		} else if strings.Contains(l.Text, bgWell) && bgWell != "" {
			others++
		}
	}
	return
}

// What you said is headed You and edged in your colour, and a paste of a
// few lines is its own text with the terminal's border residue off it.
func TestUserFillAndShortPaste(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "see\n<pasted_content>\n│ Can it keep\n│ its width?\n</pasted_content>"}, at(0))
	s.Apply(say("Yes."), at(1))
	s.Apply(headless.Result{Subtype: "success"}, at(2))
	ls := s.Render(Options{Width: 100, Now: at(3)})
	out := plain(ls)
	for _, want := range []string{"▍ You", "▍ see\n", "▍ ▤ pasted\n", "▍ Can it keep\n", "▍ its width?\n"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	yours := 0
	for _, l := range ls {
		if strings.HasPrefix(l.Text, spineYou) {
			yours++
		}
	}
	if yours != 5 {
		t.Errorf("the message is its heading and 4 rows, got %d:\n%s", yours, out)
	}
	if strings.Contains(out, "│") || strings.Contains(out, "╭") {
		t.Errorf("no frame and no border residue:\n%s", out)
	}
}

// A paste of up to 15 lines shows whole; a longer one, its first 12 and a
// link that opens the message, never a shortened excerpt.
func TestLongPasteShowsItsHead(t *testing.T) {
	var b strings.Builder
	for i := 1; i <= 20; i++ {
		fmt.Fprintf(&b, "line %d\n", i)
	}
	s := New()
	s.Apply(host.Sent{Text: "<pasted_content>\n" + b.String() + "</pasted_content>"}, at(0))
	s.Apply(headless.Result{Subtype: "success"}, at(1))
	ls := s.Render(Options{Width: 100, Now: at(2)})
	out := plain(ls)
	for _, want := range []string{"▤ pasted · 20 lines", "▍ line 12\n", "… 8 more lines · click to show all"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	if strings.Contains(out, "line 13\n") {
		t.Errorf("a long paste stops at its head:\n%s", out)
	}
	var all strings.Builder
	for _, l := range ls {
		all.WriteString(l.Text)
	}
	if !strings.Contains(all.String(), "rush:message/t1\x1b") {
		t.Error("the rest is a click away")
	}
}

// A row is only ever left clear once.
func TestSqueezedBlankRows(t *testing.T) {
	got := squeezed([]Line{{Text: "a"}, {Text: ""}, {Text: " ▏"}, {Text: "b"}})
	if len(got) != 3 {
		t.Fatalf("%d rows kept, want a, one blank and b", len(got))
	}
}

func TestQueuedUserBoxesKeepAnIntentionalSeparator(t *testing.T) {
	d := drawer{s: New(), t: &Turn{}, o: Options{Width: 80}, cw: 80, rule: true}
	d.userBox("t1", "", host.JoinQueue([]string{"one", "two"}), nil, nil, false)
	out := plain(d.lines)
	if !strings.Contains(out, "one\n▍\n▍ two") {
		t.Fatalf("queue entries need one filled blank separator: %s", out)
	}
}

func TestNumberedImagesCoverOnlyCompleteAttachmentSets(t *testing.T) {
	if !numberedImagesCover("[Image #2] then [Image #1]", 2) {
		t.Fatal("numbered image markers should cover both attachments")
	}
	if numberedImagesCover("[Image #1]", 2) {
		t.Fatal("a partial marker set must retain attachments with unknown ownership")
	}
}

// Picked, what you said has the cursor for its edge, once.
func TestPickedMessageOneEdge(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "hello"}, at(0))
	out := plain(s.Render(Options{Width: 80, Now: at(1), Selected: "t1"}))
	if strings.Contains(out, "▏▏") || strings.Contains(out, "▏▍") || !strings.Contains(out, "▏ You") {
		t.Errorf("picked:\n%s", out)
	}
}
