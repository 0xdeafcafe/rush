package host

import (
	"os"
	"path/filepath"
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

// A turn that ends with decisions puts the card on the session, which may
// still stop its agent's program and sleep: they don't need it. What you
// send next takes the card away, and so does dismissing it.
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
	if d == nil || len(d.Asks) != 3 || s.info.Needs != "" || s.info.State != "idle" {
		t.Fatalf("after the turn: %+v, needs %q", d, s.info.Needs)
	}
	s.mu.Lock()
	safe := s.runtimeIdleSafe() && s.sleepSafe() && s.info.Quiet()
	s.mu.Unlock()
	if !safe {
		t.Fatal("open decisions keep the agent's program or the host up")
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
	if s.info.Decisions == nil || NeedsDecisions(s.info.Decisions) != "2 decisions" {
		t.Fatalf("decisions %+v", s.info.Decisions)
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

// The card outlives its host sleeping or being restarted on a newer rush,
// for the same conversation only.
func TestDecisionsSurviveSleep(t *testing.T) {
	d := DecisionsOf(m1326)
	old := Info{ID: "dz", Kind: "claude", SessionID: "s1", State: "idle", Sleeping: true, Decisions: d}
	s := &server{cfg: Config{ID: "dz", Kind: "claude", SessionID: "s1", Resume: true}, info: Info{Kind: "claude"}}
	s.restoreDecisions(old)
	if s.info.Decisions == nil || s.info.Decisions.ID != d.ID {
		t.Fatalf("lost on waking: %+v", s.info.Decisions)
	}
	for _, cfg := range []Config{{ID: "dz", Kind: "claude", SessionID: "s2"}, {ID: "dz", Kind: "claude", SessionID: "s1", Fork: true}} {
		s := &server{cfg: cfg}
		if s.restoreDecisions(old); s.info.Decisions != nil {
			t.Errorf("carried into %+v", cfg)
		}
	}
}

// A card set aside while the host sleeps stays away for whoever reads its
// info, without waking it; a newer card isn't affected.
func TestDecisionsDismissedAsleep(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	const id = "s1"
	if err := os.MkdirAll(dir(id), 0o700); err != nil {
		t.Fatal(err)
	}
	write := func(decID string) {
		b, _ := jsonx.Marshal(Info{ID: id, Sleeping: true, Decisions: &event.Question{ID: decID}})
		if err := os.WriteFile(filepath.Join(dir(id), "info.json"), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("d1")
	if err := DismissAsleep(id, "d1"); err != nil {
		t.Fatal(err)
	}
	if info, err := readInfoFile(id); err != nil || info.Decisions != nil {
		t.Fatalf("the card set aside is still there: %+v %v", info.Decisions, err)
	}
	write("d2")
	if info, _ := readInfoFile(id); info.Decisions == nil || info.Decisions.ID != "d2" {
		t.Fatal("a newer card was taken away too")
	}
}

// A restarted host counts its turns on from where the last one left off.
func TestTurnsCountOnAcrossARestart(t *testing.T) {
	s := &server{}
	s.restoreTurn(Info{Turn: 7})
	if s.turn != 7 || s.info.Turn != 7 {
		t.Fatalf("turn %d, info %d", s.turn, s.info.Turn)
	}
}

// What's at stake stays apart from the topic, and leads the card's ask.
func TestDecisionContext(t *testing.T) {
	text := "Found it.\n\nDecisions:\n" +
		"1. **What to make easier to find:** Sessions that woke show no history today, and @names can't be searched. (a) history: look where the session started; (b) names: search by @name in the command bar.\n" +
		"2. Restart: (a) yes; (b) no.\n" +
		"My picks: 1a 2b."
	ds := ParseDecisions(text)
	if len(ds) != 2 {
		t.Fatalf("got %d decisions", len(ds))
	}
	if ds[0].Topic != "What to make easier to find" || !strings.HasPrefix(ds[0].Context, "Sessions that woke show no history") {
		t.Fatalf("topic %q, context %q", ds[0].Topic, ds[0].Context)
	}
	if ds[0].Options[0].Label != "history" || ds[0].Options[0].Detail != "look where the session started" {
		t.Fatalf("option %+v", ds[0].Options[0])
	}
	if ds[1].Topic != "Restart" || ds[1].Context != "" {
		t.Fatalf("topic %q, context %q", ds[1].Topic, ds[1].Context)
	}
	q := DecisionsOf(text)
	if got := q.Asks[0].Text; got != ds[0].Context+"\n\n1. What to make easier to find" {
		t.Fatalf("card text %q", got)
	}
	if got := q.Asks[1].Text; got != "2. Restart" {
		t.Fatalf("card text %q", got)
	}
	ans := DecisionAnswer(q, map[string]string{q.Asks[0].Text: "history"})
	if ans != "1a. What to make easier to find: history" {
		t.Fatalf("answer %q", ans)
	}
}

// The block a user saw as prose: one yes/no decision with what's at stake,
// and a two-letter "no" that is a label like "yes" is.
const loaderMsg = "Decisions:\n" +
	"1. **Committing the loader work:** rush's permission check stopped the lane's commit, so this needs your go-ahead. (a) yes: I commit it on perf/loader as one commit per fix, nothing pushed; (b) no: leave it uncommitted for you to look at first.\n" +
	"My picks: 1a."

func TestDecisionsLoaderBlock(t *testing.T) {
	want := []Decision{{Ref: "1", Topic: "Committing the loader work",
		Context: "rush's permission check stopped the lane's commit, so this needs your go-ahead.",
		Options: []DecisionOption{
			{Key: "a", Label: "yes", Detail: "I commit it on perf/loader as one commit per fix, nothing pushed", Recommended: true},
			{Key: "b", Label: "no", Detail: "leave it uncommitted for you to look at first"},
		}}}
	before, ds, after := DecisionsIn("The lane is done.\n\n" + loaderMsg)
	if !reflect.DeepEqual(ds, want) {
		t.Fatalf("got %+v", ds)
	}
	if before != "The lane is done." || after != "" {
		t.Fatalf("before %q, after %q", before, after)
	}
	q := DecisionsOf(loaderMsg)
	if got := DecisionAnswer(q, map[string]string{q.Asks[0].Text: "no"}); got != "1b. Committing the loader work: no" {
		t.Fatalf("answer %q", got)
	}
}

// Around the block: the heading with its picks goes, and what follows the
// picks line stays.
func TestDecisionsInKeepsWhatsAround(t *testing.T) {
	before, ds, after := DecisionsIn(m1326)
	if len(ds) != 2 || !strings.HasSuffix(before, "an \"Ask:\" line.") || after != "" {
		t.Fatalf("before %q, %d decisions, after %q", before, len(ds), after)
	}
	before, ds, after = DecisionsIn(m1335 + "\n\nSay if you'd rather wait.")
	if len(ds) != 3 || !strings.HasSuffix(before, "turned into an extra question.") || after != "Say if you'd rather wait." {
		t.Fatalf("before %q, %d decisions, after %q", before, len(ds), after)
	}
	if before, ds, _ := DecisionsIn("1. one\n2. two"); ds != nil || before != "1. one\n2. two" {
		t.Fatalf("no block: %q %+v", before, ds)
	}
}

// With no picks stated, nothing is marked.
func TestDecisionsWithoutPicks(t *testing.T) {
	ds := ParseDecisions("Decisions:\n1. **Ship:** (a) yes; (b) no.\n2. **Where:** (a) main: straight in; (b) a branch: for review.")
	if len(ds) != 2 {
		t.Fatalf("got %+v", ds)
	}
	for _, d := range ds {
		for _, o := range d.Options {
			if o.Recommended {
				t.Errorf("%s: %+v marked", d.Topic, o)
			}
		}
	}
	if o := ds[1].Options[1]; o.Label != "a branch" || o.Detail != "for review" {
		t.Errorf("option %+v", o)
	}
}

// A background task that finishes wakes the agent on its own: that turn
// answers nothing, so the open card stays unless it asks something new.
func TestDecisionsSurviveAWake(t *testing.T) {
	setup(t)
	ic := &inputConn{fakeConn: fakeConn{events: make(chan event.Event, 8)}}
	s := &server{cfg: Config{ID: "dw", Kind: "claude"}, conn: ic, clients: map[*conn]struct{}{}, pending: map[string]asked{}}
	turn := func(said string) {
		s.mu.Lock()
		defer s.mu.Unlock()
		s.info.State = "working"
		s.said(event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: said}}})
		s.onTurnEnd(ic, event.TurnEnd{Reason: "done"})
	}
	wake := func(said string) {
		s.mu.Lock()
		defer s.mu.Unlock()
		m := event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: said}}}
		s.onMessage(ic, m)
		s.onTurnEnd(ic, event.TurnEnd{Reason: "done"})
	}
	turn(m1335)
	d := s.info.Decisions
	if d == nil {
		t.Fatal("no card")
	}
	wake("The review agent finished: the rows check out.")
	if s.info.Decisions == nil || s.info.Decisions.ID != d.ID {
		t.Fatalf("a wake took the card: %+v", s.info.Decisions)
	}
	wake(m1326)
	if s.info.Decisions == nil || NeedsDecisions(s.info.Decisions) != "2 decisions" {
		t.Fatalf("a wake that asks again: %+v", s.info.Decisions)
	}
	if err := s.send("go on", nil, false); err != nil {
		t.Fatal(err)
	}
	turn("Done.")
	if s.info.Decisions != nil {
		t.Fatalf("what you sent leaves the card: %+v", s.info.Decisions)
	}
}
