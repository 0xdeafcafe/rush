package convo

import (
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// Hit is one search result: where it is and the text around the match.
type Hit struct {
	Ref     string // the turn or step it jumps to
	Turn    int
	Who     string // you, the agent (WhoAgent), or the tool
	Snippet string
	At      time.Time // when it was said, or the step ran
	Score   int       // how closely it matches: higher is closer
}

type query struct {
	words    []string
	kinds    map[string]bool // failed, edit, cmd, read, you, agent
	file     string
	from, to int // turn range; 0 means any
	unknown  []string
}

var knownKinds = map[string]bool{"failed": true, "edit": true, "cmd": true, "read": true, "you": true, WhoAgent: true}

// WhoAgent is who a hit in what the agent said is from, and the word that
// finds only those.
const WhoAgent = "agent"

// whoClaude is is:claude, kept for searches typed before rush ran other
// agents, until is:agent has been the word a while.
const whoClaude = "claude" // migration: is:claude, the word before is:agent

func parseQuery(q string) query {
	p := query{kinds: map[string]bool{}}
	for _, f := range strings.Fields(strings.ToLower(q)) {
		switch {
		case strings.HasPrefix(f, "is:"):
			k := strings.TrimPrefix(f, "is:")
			if k == whoClaude {
				k = WhoAgent
			}
			if !knownKinds[k] {
				p.unknown = append(p.unknown, f)
				continue
			}
			p.kinds[k] = true
		case strings.HasPrefix(f, "file:"):
			p.file = strings.TrimPrefix(f, "file:")
		case strings.HasPrefix(f, "turn:"):
			r := strings.TrimPrefix(f, "turn:")
			a, b, found := strings.Cut(r, "-")
			p.from, _ = strconv.Atoi(a)
			p.to = p.from
			if found {
				p.to, _ = strconv.Atoi(b)
				if b == "" {
					p.to = 1 << 30 // turn:5- runs to the end
				}
			}
			if p.to < p.from {
				p.from, p.to = p.to, p.from
			}
		default:
			p.words = append(p.words, f)
		}
	}
	return p
}

// matches reports whether every word is in text, loosely (wordMatch).
func (p query) matches(text string) bool {
	_, ok := p.score(text)
	return ok
}

// score is how closely text matches every word, and whether it does: each
// word's match, and more when the words are there as typed, side by side.
func (p query) score(text string) (int, bool) {
	total := 0
	for _, w := range p.words {
		s, _, _ := wordMatch(text, w)
		if s == 0 {
			return 0, false
		}
		total += s
	}
	if len(p.words) > 1 {
		if i, _ := findFold(text, strings.Join(p.words, " ")); i >= 0 {
			total += scorePhrase
		}
	}
	return total, true
}

// findFold finds w in s ignoring case, rune by rune, and returns the byte
// span of the match in s itself (lowercasing can change a string's byte
// length, so an index into a lowercased copy doesn't fit the original).
func findFold(s, w string) (start, end int) {
	if w == "" {
		return 0, 0
	}
	for i := range s {
		j, k := i, 0
		for k < len(w) && j < len(s) {
			a, na := utf8.DecodeRuneInString(s[j:])
			b, nb := utf8.DecodeRuneInString(w[k:])
			if unicode.ToLower(a) != unicode.ToLower(b) {
				break
			}
			j, k = j+na, k+nb
		}
		if k == len(w) {
			return i, j
		}
	}
	return -1, -1
}

// Search finds turns and steps that match q: plain words anywhere, narrowed
// by is:failed, is:edit, is:cmd, is:read, is:you, is:agent, file:<part of
// a path> and turn:12 or turn:10-13.
func (s *Session) Search(q string) []Hit {
	p := parseQuery(q)
	if len(p.words) == 0 && len(p.kinds) == 0 && p.file == "" && p.from == 0 {
		return nil
	}
	steps := p.kinds["failed"] || p.kinds["edit"] || p.kinds["cmd"] || p.kinds["read"] || p.file != ""
	var hits []Hit
	for _, t := range s.Turns {
		if p.from > 0 && (t.N < p.from || t.N > p.to) {
			continue
		}
		ref := fmt.Sprintf("t%d", t.N)
		if !steps && !p.kinds[WhoAgent] && (len(p.words) > 0 || p.kinds["you"] || p.from > 0) {
			// What you said counts for more than what was said back.
			if sc, ok := p.score(t.Prompt); ok {
				hits = append(hits, Hit{Ref: ref, Turn: t.N, Who: "you", Snippet: snippet(t.Prompt, p.words), At: t.Start, Score: sc + 20})
			}
		}
		if p.kinds["you"] {
			continue
		}
		for _, it := range t.Items {
			switch it.Kind {
			case KText:
				if steps || len(p.words) == 0 && !p.kinds[WhoAgent] {
					continue
				}
				sc, ok := p.score(it.Text)
				if !ok {
					continue
				}
				hits = append(hits, Hit{Ref: ref, Turn: t.N, Who: WhoAgent, Snippet: snippet(it.Text, p.words), At: textAt(it, t), Score: sc + 10})
			case KStep:
				if p.kinds[WhoAgent] {
					continue
				}
				for _, st := range append([]*Step{it.Step}, it.Step.Children...) {
					if h, ok := p.step(st, ref); ok {
						h.Turn, h.At = t.N, firstTime(st.Start, t.Start)
						hits = append(hits, h)
					}
				}
			}
		}
	}
	return hits
}

func (p query) step(st *Step, turnRef string) (Hit, bool) {
	if hidden(st) {
		return Hit{}, false
	}
	x := st.in()
	// A search's or glob's path is where it looked, not a file it's about.
	file := x.Path
	if k := st.kind(); k == tool.Search || k == tool.Glob {
		file = ""
	}
	g := glyphFor(st)
	switch {
	case p.kinds["failed"] && st.Status != Failed:
		return Hit{}, false
	case p.kinds["edit"] && g != "✎":
		return Hit{}, false
	case p.kinds["cmd"] && g != "$":
		return Hit{}, false
	case p.kinds["read"] && g != "◧":
		return Hit{}, false
	case p.file != "" && !strings.Contains(strings.ToLower(x.Path+" "+x.Command), p.file):
		return Hit{}, false
	}
	label := firstNonEmpty(x.Description, x.Command, file, x.Pattern, x.URL, x.Query, st.Tool)
	hay := label + "\n" + x.Command + "\n" + file + "\n" + st.Output
	sc, ok := p.score(hay)
	if !ok {
		return Hit{}, false
	}
	snip := oneLine(label)
	if len(p.words) > 0 && !p.matches(label) {
		snip = oneLine(label) + " · " + snippet(st.Output, p.words)
	}
	return Hit{Ref: turnRef + ":s:" + st.ID, Who: st.Tool, Snippet: snip, Score: sc}, true
}

// textAt is when the agent said it: the turn's end for its final words,
// else the turn's start, all that's kept.
func textAt(it *Item, t *Turn) time.Time {
	if it.Answer {
		return firstTime(t.End, t.Start)
	}
	return t.Start
}

// snippet is the text around the first word found, on one line.
func snippet(text string, words []string) string {
	t := oneLine(text)
	if len(words) == 0 {
		return t
	}
	i := -1
	for _, w := range words {
		if _, i, _ = wordMatch(t, w); i >= 0 {
			break
		}
	}
	if i < 0 {
		return t
	}
	// About 30 characters before it, starting on a whole character.
	start := i
	for n := 0; n < 30 && start > 0; n++ {
		_, size := utf8.DecodeLastRuneInString(t[:start])
		start -= size
	}
	out := t[start:]
	if start > 0 {
		out = "…" + out
	}
	return out
}

// SearchView draws the hits as rows, with the first word lit.
func (s *Session) SearchView(q string, o Options) []Line {
	w := min(o.Width, o.rowCap())
	// The same query over an unchanged session gives the same hits.
	key := fmt.Sprint(q, "\x00", s.stepVer, len(s.Turns), s.Last.UnixNano())
	if s.searchKey != key {
		s.searchHits, s.searchKey = s.Search(q), key
	}
	hits := s.searchHits
	var out []Line
	head := fmt.Sprintf("%d matches", len(hits))
	if strings.TrimSpace(q) == "" {
		head = "type to search · is:failed is:edit is:cmd is:read is:you is:agent file:<name> turn:10-13"
	}
	if u := parseQuery(q).unknown; len(u) > 0 {
		head += " · " + strings.Join(u, " ") + " isn't a filter (is:failed is:edit is:cmd is:read is:you is:agent)"
	}
	out = append(out, Line{Text: row("", "  "+dim(head), dim("enter jumps · esc closes"), o.Width, w)}, Line{Text: ""})
	words := parseQuery(q).words
	for _, h := range hits {
		who := dim(fmt.Sprintf("%-8s", h.Who))
		if h.Who == "you" {
			who = paint(cWhite, fmt.Sprintf("%-8s", h.Who))
		}
		snip := h.Snippet
		if len(words) > 0 {
			if _, i, j := wordMatch(snip, words[0]); i >= 0 {
				snip = sub(snip[:i]) + paint(cYellow+bold, snip[i:j]) + sub(snip[j:])
			} else {
				snip = sub(snip)
			}
		} else {
			snip = sub(snip)
		}
		left := "  " + dim(fmt.Sprintf("#%-4d", h.Turn)) + " " + who + " " + snip
		if h.Ref == o.Selected {
			left = cursor() + strings.TrimPrefix(left, " ")
		}
		out = append(out, Line{Text: row("", left, "", o.Width, w), Ref: h.Ref})
	}
	return out
}

// ParentRef is the ref of the step a subagent's step belongs to, or "" when
// ref isn't a subagent's step. Opening it shows the step.
func (s *Session) ParentRef(ref string) string {
	turn, id, ok := strings.Cut(ref, ":s:")
	if !ok {
		return ""
	}
	if st := s.byID[id]; st != nil && st.parent != nil {
		return turn + ":s:" + st.parent.ID
	}
	return ""
}
