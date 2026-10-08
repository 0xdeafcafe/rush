package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestQueuedMessagesRenderWithoutDeliveryEnvelope(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "start"}, at(0))
	joined := host.JoinQueue([]string{"first message", "second message", "[Image #1] look here"})
	s.Apply(host.Sent{Text: joined, Images: []string{"/missing/shot.png"}}, at(1))
	lines := s.Render(Options{Width: 80, Now: at(2)})
	out := plain(lines)
	for _, unwanted := range []string{"mid-turn", "queued while", "[Message 1 of"} {
		if strings.Contains(out, unwanted) {
			t.Fatalf("delivery scaffolding leaked: %s", out)
		}
	}
	for _, wanted := range []string{"first message", "second message", "look here", "Image #1"} {
		if !strings.Contains(out, wanted) {
			t.Fatalf("lost %q: %s", wanted, out)
		}
	}
	if !strings.Contains(out, "you\n▌\n▌  second message") {
		t.Fatalf("messages are not separated: %s", out)
	}
	messages := s.UserMessages()
	if messages[len(messages)-1].Text != joined {
		t.Fatal("presentation changed original message")
	}
	if !strings.Contains(strings.Join(func() []string {
		var a []string
		for _, l := range lines {
			a = append(a, l.Text)
		}
		return a
	}(), ""), "/image/1") {
		t.Fatal("image link lost")
	}
	if strings.Count(out, "Image #1") != 1 || strings.Contains(out, "shot.png") {
		t.Fatalf("numbered image should render once without a duplicate filename chip: %s", out)
	}
}

func TestQueuedEnvelopeRequiresExactStructure(t *testing.T) {
	for _, text := range []string{"2 messages, queued while you worked. Each is its own message; take them in order.\n\n[Message 2 of 2]\ntext", "[Message 1 of 2]\nordinary user text", "2 messages, not a queue\n\n[Message 1 of 2]\ntext"} {
		if queuedMessages(text) != nil {
			t.Fatalf("ordinary or malformed message changed: %q", text)
		}
	}
}

func TestFailureRowsAreTintedWithoutBorders(t *testing.T) {
	d := drawer{s: New(), t: &Turn{}, o: Options{Width: 100}, cw: 100}
	d.card(card{kind: "build", what: "go build", fails: []failure{{name: "main.go", msg: "undefined: thing"}}}, 4)
	out := plain(d.lines)
	if strings.ContainsAny(out, "╭╰│┌└") || !strings.Contains(out, "undefined: thing") {
		t.Fatalf("bad failure presentation: %s", out)
	}
	for _, line := range d.lines {
		if !strings.Contains(line.Text, bgFailure) || cellw.String(line.Text) != 100 {
			t.Fatal("failure tint must span entire row")
		}
	}
}

func TestExpandedAgentMessageFormatsMarkdown(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "send it"}, at(0))
	body := "## Tasks\n\n| Task | Owner | State |\n|---|---|---|\n| Build | Alex | Ready |\n\n- **Keep this**\n- Then run tests"
	s.Apply(toolUse("m", "SendMessage", map[string]any{"to": "peer", "message": body}), at(1))
	s.Apply(toolResult("m", "ok", false, nil), at(2))
	for _, width := range []int{50, 100} {
		lines := s.Render(Options{Width: width, Now: at(3), Open: map[string]bool{"t1:s:m": true}})
		out := plain(lines)
		for _, bad := range []string{"## Tasks", "|---", "**Keep this**", "| Task |"} {
			if strings.Contains(out, bad) {
				t.Fatalf("raw Markdown: %s", out)
			}
		}
		for _, want := range []string{"Tasks", "Build", "Alex", "Ready", "• Keep this", "• Then run tests"} {
			if !strings.Contains(out, want) {
				t.Fatalf("missing %s: %s", want, out)
			}
		}
		for _, line := range lines {
			if cellw.String(line.Text) > width {
				t.Fatal("message overflow")
			}
		}
	}
}
