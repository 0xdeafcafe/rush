package host

import (
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// An agent that needs you to decide something ends its turn with a
// decisions block, read here without a model:
//
//	Decisions:
//	1. **Topic:** (a) first option; (b) second option.
//	2. **Topic:** (a) …; (b) ….
//	My picks: 1a 2b.
//
// Only the last run of numbered items counts, so numbered lists earlier in
// the message (findings, steps) are left alone. Picks come from any line
// that says "pick" or "picks", or from "(recommended)" on an option. The
// host shows it as a card (Info.Decisions) and your answers go back as the
// next message (DecisionAnswer). It's read at every turn's end, so a message
// with no "(a)" in it costs one search.

// decisionsMost is how many decisions a card takes.
const decisionsMost = 9

// decisionsPrompt tells the agent how to ask for decisions so rush can draw
// them as a card: kept short, as every session carries it.
const decisionsPrompt = `When you need the user to decide something, end your reply with a decisions block, which rush shows as a card:
Decisions:
1. **Topic:** (a) option; (b) option.
My picks: 1a 2b.
One numbered line per decision, a yes/no as (a) yes; (b) no. Answers come back as ` + "`1a. Topic: option`" + ` or ` + "`2: their words`" + `.`

// Decision is one numbered item of a decisions block.
type Decision struct {
	Ref     string // its number in the message
	Topic   string
	Options []DecisionOption
}

// DecisionOption is one lettered option of a Decision.
type DecisionOption struct {
	Key         string // its letter
	Label       string
	Detail      string
	Recommended bool
}

var (
	decisionItem  = regexp.MustCompile(`^\s*(\d+)[.)]\s+(.*)$`)
	decisionGoOn  = regexp.MustCompile(`^\s{2,}\S`)
	decisionOpt   = regexp.MustCompile(`\(([a-z])\)\s*`)
	decisionPick  = regexp.MustCompile(`(?i)\bpicks?\b`)
	decisionRef   = regexp.MustCompile(`\b(\d+)([a-z])\b`)
	decisionRec   = regexp.MustCompile(`(?i)\((recommended|my pick)\)`)
	decisionRecIn = regexp.MustCompile(`(?i)\s*\((recommended|my pick)\)`)
	// A short lead before ": ", "; ", " — " or ". " is the label, and the
	// rest explains it.
	decisionSplit = regexp.MustCompile(`^(.{3,70}?)(?::\s|;\s|\s[—–-]\s|\.\s)(.+)$`)
)

// plainText is card text with no markdown: inline code loses its backticks.
func plainText(s string) string { return strings.ReplaceAll(s, "`", "") }

// cleanText is s without bold markers, its whitespace collapsed.
func cleanText(s string) string {
	s = strings.ReplaceAll(strings.ReplaceAll(s, "**", ""), "__", "")
	return strings.Join(strings.Fields(s), " ")
}

// statedPicks are the picks text states, by decision number: "my picks:
// 1a 2b", "(my picks are 1a, 2a, go)", "\"1b 2a\" are my picks".
func statedPicks(text string) map[string]string {
	picks := map[string]string{}
	if !strings.Contains(strings.ToLower(text), "pick") {
		return picks
	}
	for l := range strings.Lines(text) {
		if !decisionPick.MatchString(l) {
			continue
		}
		for _, m := range decisionRef.FindAllStringSubmatch(l, -1) {
			picks[m[1]] = m[2]
		}
	}
	return picks
}

// ParseDecisions is the last decisions block in text, or none when its last
// run of numbered items has no lettered options.
func ParseDecisions(text string) []Decision {
	if !strings.Contains(text, "(a)") {
		return nil // every decision has an (a)
	}
	type item struct{ ref, body string }
	// Runs of numbered items: blank lines stay inside a run, any other line
	// ends it, and indented lines carry an item on. Only the final run
	// counts: when it's no decisions block ("Land it?\n1. Yes\n2. No"), an
	// earlier lettered list mustn't stand in for it.
	var last, block []item
	for l := range strings.Lines(text) {
		l = strings.TrimRight(l, "\r\n")
		if m := decisionItem.FindStringSubmatch(l); m != nil {
			block = append(block, item{m[1], m[2]})
		} else if len(block) > 0 && decisionGoOn.MatchString(l) {
			block[len(block)-1].body += " " + strings.TrimSpace(l)
		} else if strings.TrimSpace(l) != "" && len(block) > 0 {
			last, block = block, nil
		}
	}
	if len(block) > 0 {
		last = block
	}
	var out []Decision
	var picks map[string]string
	for _, it := range last {
		if !strings.Contains(it.body, "(a)") {
			continue
		}
		if picks == nil {
			picks = statedPicks(text)
		}
		if d, ok := decision(it.ref, it.body, picks[it.ref]); ok {
			out = append(out, d)
		}
	}
	return out
}

// decision reads one numbered item's topic and lettered options; picked is
// the letter picked for it, if any.
func decision(ref, body, picked string) (Decision, bool) {
	// "Topic: ", "a", "first; ", "b", "second."
	at := decisionOpt.FindAllStringSubmatchIndex(body, -1)
	if len(at) == 0 {
		return Decision{}, false
	}
	topic := plainText(strings.TrimRight(cleanText(body[:at[0][0]]), ": \t"))
	// Letters run a, b, c…: one out of sequence is a mention inside the
	// option before it.
	var opts [][2]string
	for i, m := range at {
		key, end := body[m[2]:m[3]], len(body)
		if i+1 < len(at) {
			end = at[i+1][0]
		}
		text := body[m[1]:end]
		switch {
		case key == string(rune('a'+len(opts))):
			opts = append(opts, [2]string{key, text})
		case len(opts) > 0:
			opts[len(opts)-1][1] += "(" + key + ") " + text
		default:
			return Decision{}, false
		}
	}
	if topic == "" || len(opts) < 2 {
		return Decision{}, false
	}
	// Answers name an option by its label, so labels stay distinct: when
	// shortening would repeat one, they're kept whole; an option whose
	// whole text repeats an earlier one offers nothing new and is left out.
	build := func(split bool) []DecisionOption {
		out := make([]DecisionOption, len(opts))
		for i, o := range opts {
			out[i] = decisionOption(o[0], o[1], picked == o[0], split)
		}
		return out
	}
	list := build(true)
	if !distinctLabels(list) {
		list = build(false)
	}
	seen := map[string]bool{}
	kept := list[:0]
	for _, o := range list {
		if !seen[o.Label] {
			seen[o.Label] = true
			kept = append(kept, o)
		}
	}
	if len(kept) < 2 {
		return Decision{}, false
	}
	return Decision{Ref: ref, Topic: topic, Options: kept}, true
}

func decisionOption(key, raw string, picked, split bool) DecisionOption {
	text := cleanText(raw)
	if n := len(text); n > 0 && strings.ContainsRune(";,.", rune(text[n-1])) {
		text = text[:n-1]
	}
	o := DecisionOption{Key: key, Recommended: picked || decisionRec.MatchString(text)}
	text = strings.TrimSpace(decisionRecIn.ReplaceAllString(text, ""))
	o.Label = plainText(text)
	// Never split inside inline code.
	if m := decisionSplit.FindStringSubmatch(text); split && m != nil && strings.Count(m[1], "`")%2 == 0 {
		o.Label, o.Detail = plainText(m[1]), plainText(m[2])
	}
	return o
}

func distinctLabels(list []DecisionOption) bool {
	seen := map[string]bool{}
	for _, o := range list {
		if seen[o.Label] {
			return false
		}
		seen[o.Label] = true
	}
	return true
}

// DecisionsOf is text's decisions block as a question card, nil when it has
// none: each decision an ask keyed by its number, each option a choice
// keyed by its letter, the picks marked.
func DecisionsOf(text string) *event.Question {
	ds := ParseDecisions(text)
	if len(ds) == 0 {
		return nil
	}
	q := &event.Question{ID: "decisions-" + strconv.FormatInt(time.Now().UnixNano(), 36)}
	for _, d := range ds[:min(len(ds), decisionsMost)] {
		a := event.Ask{ID: d.Ref, Header: d.Topic, Text: d.Ref + ". " + d.Topic}
		for _, o := range d.Options {
			a.Options = append(a.Options, event.Choice{Key: o.Key, Label: o.Label, Description: o.Detail, Picked: o.Recommended})
		}
		q.Asks = append(q.Asks, a)
	}
	return q
}

// DecisionAnswer is the message that answers q, in its own numbering, one
// paragraph a decision: "1a. Topic: label" for an option, "2: text" for
// words of your own. answers are by ask text, as the question card keeps
// them; one left unanswered is left out.
func DecisionAnswer(q *event.Question, answers map[string]string) string {
	var out []string
	for _, a := range q.Asks {
		ans := strings.TrimSpace(answers[a.Text])
		if ans == "" {
			continue
		}
		line := a.ID + ": " + ans
		for _, o := range a.Options {
			if o.Label == ans {
				line = a.ID + o.Key + ". " + a.Header + ": " + ans
				break
			}
		}
		out = append(out, line)
	}
	return strings.Join(out, "\n\n")
}

// dropDecisions takes the decisions card away, and what it said the
// session needs. Called with mu held.
func (s *server) dropDecisions() {
	if s.info.Decisions == nil {
		return
	}
	if s.info.Needs == needsDecisions(s.info.Decisions) {
		s.info.Needs = ""
	}
	s.info.Decisions = nil
}

// needsDecisions is what the list says a session with decisions needs.
func needsDecisions(q *event.Question) string {
	if len(q.Asks) == 1 {
		return "decide: " + q.Asks[0].Header
	}
	return strconv.Itoa(len(q.Asks)) + " decisions"
}
