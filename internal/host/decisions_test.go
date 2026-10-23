package host

import (
	"os"
	"reflect"
	"strconv"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// The decisions block's tests are Ultimate's (test/decisions.test.ts), with
// the messages it was built from.

// Message 1326, whose card offered only a free-text field.
const m1326 = "**Branch prompts in boxer's project context:** I read boxer's root (#1) from here.\n\n" +
	"**My take:** you're right, they don't belong there in full.\n" +
	"- **Already covered:** each branch's abstract already opens with an \"Ask:\" line.\n\n" +
	"Decisions (my picks: 1a 2a):\n" +
	"1. **Branch starts in the context:** (a) one-line intent plus breadcrumb; (b) drop them, rely on abstracts; (c) leave as is.\n" +
	"2. **Boxer's copied `agents/ultimate.md`:** (a) assess a way for projects to add tools to @ultimate without forking it; (b) leave it."

// Message 1335: numbered findings before the decisions block, the picks on
// a line of their own.
const m1335 = "**The main cause is my message format, not the card.**\n" +
	"1. **Stated picks read as decided.** The prompt says to leave out \"anything already decided\".\n" +
	"2. **Extra questions:** a trailing \"go\" or \"Build it?\" turned into an extra question.\n\n" +
	"Decisions:\n" +
	"1. **Scope:** (a) the full recommendation; (b) the easier alternative.\n" +
	"2. **Keyboard browser test:** (a) add one that answers a 3-question card with the keyboard; (b) fast tests only.\n" +
	"3. **Answer format:** (a) `1a. Topic: label` lines; (b) keep `Q:/A:`.\n\n" +
	"My picks: 1a 2a 3a."

func labels(d Decision) []string {
	var out []string
	for _, o := range d.Options {
		out = append(out, o.Label)
	}
	return out
}

func TestDecisionsReadOptionsAndPicks(t *testing.T) {
	got := ParseDecisions(m1326)
	want := []Decision{
		{Ref: "1", Topic: "Branch starts in the context", Options: []DecisionOption{
			{Key: "a", Label: "one-line intent plus breadcrumb", Recommended: true},
			{Key: "b", Label: "drop them, rely on abstracts"},
			{Key: "c", Label: "leave as is"},
		}},
		{Ref: "2", Topic: "Boxer's copied agents/ultimate.md", Options: []DecisionOption{
			{Key: "a", Label: "assess a way for projects to add tools to @ultimate without forking it", Recommended: true},
			{Key: "b", Label: "leave it"},
		}},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %+v", got)
	}
}

func TestDecisionsTakeTheLastBlock(t *testing.T) {
	ds := ParseDecisions(m1335)
	var topics []string
	for _, d := range ds {
		topics = append(topics, d.Topic)
		if !d.Options[0].Recommended || d.Options[1].Recommended {
			t.Errorf("%s: picks %+v", d.Topic, d.Options)
		}
	}
	if !reflect.DeepEqual(topics, []string{"Scope", "Keyboard browser test", "Answer format"}) {
		t.Fatalf("topics %q", topics)
	}
	// A colon inside inline code splits no label.
	if o := ds[2].Options[0]; o != (DecisionOption{Key: "a", Label: "1a. Topic: label lines", Recommended: true}) {
		t.Errorf("inline code: %+v", o)
	}
}

func TestDecisionsOutOfSequenceLetter(t *testing.T) {
	ds := ParseDecisions("1. **Scope:** (a) ports and teardown; (b) onboarding only; (c) (a) without teardown.\nMy picks: 1c.")
	if len(ds) != 1 || !reflect.DeepEqual(labels(ds[0]), []string{"ports and teardown", "onboarding only", "(a) without teardown"}) || !ds[0].Options[2].Recommended {
		t.Errorf("got %+v", ds)
	}
}

func TestDecisionsRecommendedAndPicksInProse(t *testing.T) {
	ds := ParseDecisions("Decisions (my picks are 1b, 2a, go):\n1. **A:** (a) one; (b) two (recommended).\n2. **B:** (a) yes; (b) no.")
	if len(ds) != 2 {
		t.Fatalf("got %+v", ds)
	}
	if o := ds[0].Options; o[0].Label != "one" || o[0].Recommended || o[1].Label != "two" || !o[1].Recommended {
		t.Errorf("first %+v", o)
	}
	if !ds[1].Options[0].Recommended {
		t.Errorf("second %+v", ds[1].Options)
	}
}

func TestDecisionsNeverFromAnEarlierReport(t *testing.T) {
	if ds := ParseDecisions("Checks completed:\n1. Verified: (a) unit tests pass; (b) build passes.\n\nLand it?\n1. Yes\n2. Change something first"); ds != nil {
		t.Errorf("got %+v", ds)
	}
}

func TestDecisionLabelsStayDistinct(t *testing.T) {
	keep := ParseDecisions("1. **Keep:** (a) Keep: summaries only; (b) Keep: full history.")
	if !reflect.DeepEqual(labels(keep[0]), []string{"Keep: summaries only", "Keep: full history"}) {
		t.Errorf("shortened into one: %q", labels(keep[0]))
	}
	// Compared once inline code has lost its backticks.
	code := ParseDecisions("1. **Keep:** (a) `Keep`: summaries only; (b) Keep: full history.")
	if !reflect.DeepEqual(labels(code[0]), []string{"Keep: summaries only", "Keep: full history"}) {
		t.Errorf("with code: %q", labels(code[0]))
	}
	// Identical options leave one, so a choice never maps to another's letter.
	if ds := ParseDecisions("1. **Mode:** (a) Keep; (b) Keep."); ds != nil {
		t.Errorf("one option left: %+v", ds)
	}
	mode := ParseDecisions("1. **Mode:** (a) Keep; (b) Keep; (c) Drop.")
	if o := mode[0].Options; len(o) != 2 || o[0].Key != "a" || o[1].Key != "c" || o[1].Label != "Drop" {
		t.Errorf("mode %+v", o)
	}
	q := DecisionsOf("1. **Keep:** (a) Keep: summaries only; (b) Keep: full history.")
	if got := DecisionAnswer(q, map[string]string{q.Asks[0].Text: q.Asks[0].Options[1].Label}); got != "1b. Keep: Keep: full history" {
		t.Errorf("answer %q", got)
	}
}

func TestNoDecisionsInPlainLists(t *testing.T) {
	for _, s := range []string{"Land it?\n1. Yes\n2. Change something first", "Landed `abc1234`. Nothing to decide.", "Steps:\n1. Read (a) the code.\n2. Run it.", ""} {
		if ds := ParseDecisions(s); ds != nil {
			t.Errorf("%q: %+v", s, ds)
		}
	}
}

func TestDecisionCard(t *testing.T) {
	q := DecisionsOf(m1326)
	if q == nil || len(q.Asks) != 2 || q.Asks[0].ID != "1" || q.Asks[0].Header != "Branch starts in the context" || len(q.Asks[0].Options) != 3 {
		t.Fatalf("card %+v", q)
	}
	if o := q.Asks[0].Options; !o[0].Picked || o[1].Picked || o[2].Key != "c" {
		t.Errorf("options %+v", o)
	}
	// In the message's own numbering, typed text as "n: text", the
	// unanswered left out.
	got := DecisionAnswer(q, map[string]string{q.Asks[0].Text: "one-line intent plus breadcrumb", q.Asks[1].Text: "only for @ultimate, not reviewers"})
	if want := "1a. Branch starts in the context: one-line intent plus breadcrumb\n\n2: only for @ultimate, not reviewers"; got != want {
		t.Errorf("answer %q", got)
	}
	if got := DecisionAnswer(q, map[string]string{q.Asks[1].Text: "leave it"}); got != "2b. Boxer's copied agents/ultimate.md: leave it" {
		t.Errorf("one answered %q", got)
	}
	var many strings.Builder
	for i := range 12 {
		many.WriteString(strconv.Itoa(i+1) + ". **T" + strconv.Itoa(i+1) + ":** (a) yes; (b) no.\n")
	}
	if q := DecisionsOf(many.String()); q == nil || len(q.Asks) != decisionsMost {
		t.Errorf("at most %d: %+v", decisionsMost, q)
	}
}

// Ultimate's real reply-expected messages with a decisions block, and the
// card each should give: n decisions, opts options each, rec the picked
// option of each (null: none stated).
func TestDecisionCases(t *testing.T) {
	b, err := os.ReadFile("testdata/decisions.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		ID   int    `json:"id"`
		N    int    `json:"n"`
		Opts []int  `json:"opts"`
		Rec  []*int `json:"rec"`
		Text string `json:"text"`
	}
	if err := jsonx.Unmarshal(b, &cases); err != nil {
		t.Fatal(err)
	}
	for _, c := range cases {
		ds := ParseDecisions(c.Text)
		if len(ds) != c.N {
			t.Errorf("%d: %d decisions, want %d", c.ID, len(ds), c.N)
			continue
		}
		for i, d := range ds {
			rec := -1
			for j, o := range d.Options {
				if o.Recommended {
					rec = j
				}
			}
			want := -1
			if c.Rec[i] != nil {
				want = *c.Rec[i]
			}
			if len(d.Options) != c.Opts[i] || rec != want {
				t.Errorf("%d.%s: %d options picking %d, want %d picking %d", c.ID, d.Ref, len(d.Options), rec, c.Opts[i], want)
			}
		}
	}
}

// A turn that ends with decisions puts the card on the session and says it
// needs you; what you send next takes it away, and so does dismissing it.
func TestDecisionsOnTheSession(t *testing.T) {
	setup(t)
	ic := &inputConn{fakeConn: fakeConn{events: make(chan event.Event, 8)}}
	s := &server{cfg: Config{ID: "dec", Kind: "claude"}, conn: ic, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	turn := func(said, reason string) {
		s.mu.Lock()
		defer s.mu.Unlock()
		s.info.State = "working"
		if said != "" {
			s.said(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: said}}})
		}
		s.onTurnEnd(ic, event.TurnEnd{Reason: reason})
	}
	turn(m1335, "done")
	d := s.info.Decisions
	if d == nil || len(d.Asks) != 3 || s.info.Needs != "3 decisions" || s.info.State != "idle" {
		t.Fatalf("after the turn: %+v, needs %q", d, s.info.Needs)
	}
	if err := s.send(DecisionAnswer(d, map[string]string{d.Asks[0].Text: d.Asks[0].Options[0].Label}), nil, false); err != nil {
		t.Fatal(err)
	}
	if s.info.Decisions != nil || s.info.Needs != "" || len(ic.got) != 1 || ic.got[0].Text != "1a. Scope: the full recommendation" {
		t.Fatalf("answered: %+v, needs %q, sent %+v", s.info.Decisions, s.info.Needs, ic.got)
	}
	// The next turn said nothing new: last turn's block isn't asked again.
	turn("", "done")
	if s.info.Decisions != nil {
		t.Fatal("asked again from an earlier turn")
	}
	turn(m1326, "interrupted")
	if s.info.Decisions != nil {
		t.Fatal("asked from an interrupted turn")
	}
	turn(m1326, "done")
	if s.info.Needs != "2 decisions" {
		t.Fatalf("needs %q", s.info.Needs)
	}
	if err := s.do(op{Op: "decisions_dismiss", ID: "someone else's"}); err != nil || s.info.Decisions == nil {
		t.Fatalf("another card's dismissal: %v", err)
	}
	if err := s.do(op{Op: "decisions_dismiss", ID: s.info.Decisions.ID}); err != nil || s.info.Decisions != nil || s.info.Needs != "" {
		t.Fatalf("dismissed: %v, %+v", err, s.info.Decisions)
	}
	if len(ic.got) != 1 {
		t.Errorf("dismissing sent %+v", ic.got[1:])
	}
}
