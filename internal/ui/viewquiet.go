package ui

import (
	"strings"
	"time"
	"unicode"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A view the agent has stopped keeping goes grey, so the eye goes to the
// conversation instead: one the agent left alone for a whole turn, and
// one that only says what the reply beside it says. rush's own rows at
// its top (the subagents) stay bright: they're live whatever the agent
// does.

// viewQuietKey is what a view's greying is worked out from.
type viewQuietKey struct {
	name    string
	updated time.Time
	turns   int
	end     time.Time
}

// quietWhy is why the session's view v shows grey, in a few words for
// the panel's foot; "" when it doesn't. Kept until the view or the
// session's turns change.
func (st *viewsState) quietWhy(s *convo.Session, v *host.View) string {
	if s == nil || v.By != "" || v.Pinned {
		return "" // a plugin's or one you brought back: theirs and yours
	}
	k := viewQuietKey{name: v.Name, updated: v.Updated, turns: len(s.Turns)}
	if n := len(s.Turns); n > 0 {
		k.end = s.Turns[n-1].End
	}
	if st.quietKey == k {
		return st.quietMemo
	}
	st.quietKey, st.quietMemo = k, viewQuiet(s.Turns, v)
	return st.quietMemo
}

// viewQuiet is quietWhy worked out from the session's turns.
func viewQuiet(turns []*convo.Turn, v *host.View) string {
	var made *convo.Turn // the turn the view was last set in
	for _, t := range turns {
		if t.Start.After(v.Updated) {
			if !t.Live && !t.End.IsZero() {
				return "not updated for a turn"
			}
			break
		}
		made = t
	}
	if made == nil || made.Live {
		return "" // its reply is still being written
	}
	var reply strings.Builder
	for _, it := range made.Items {
		if it.Kind == convo.KText {
			reply.WriteString(it.Text)
			reply.WriteByte(' ')
		}
	}
	if repeats(viewWords(v), reply.String()) {
		return "same as the reply"
	}
	return ""
}

// repeats is whether most of a view's words, of eight or more, are the
// reply's too.
func repeats(words map[string]bool, reply string) bool {
	if len(words) < 8 {
		return false
	}
	in := wordSet(reply)
	n := 0
	for w := range words {
		if in[w] {
			n++
		}
	}
	return n*10 >= len(words)*7
}

// viewWords are the words the agent wrote in v.
func viewWords(v *host.View) map[string]bool {
	var b strings.Builder
	add := func(ss ...string) {
		for _, s := range ss {
			b.WriteString(s)
			b.WriteByte(' ')
		}
	}
	add(v.Title, v.Summary, v.Note)
	for _, g := range v.Groups {
		add(g.Title, g.Text)
		for i := range g.Rows {
			add(g.Rows[i].Text, g.Rows[i].Meta)
		}
	}
	var items func([]host.Item)
	items = func(its []host.Item) {
		for i := range its {
			add(its[i].Text, its[i].Meta, its[i].From, its[i].To)
			add(its[i].Cells...)
			items(its[i].Items)
		}
	}
	for i := range v.Blocks {
		add(v.Blocks[i].Title, v.Blocks[i].Text)
		items(v.Blocks[i].Items)
	}
	return wordSet(b.String())
}

// wordSet is s's words of four letters or more, lower case.
func wordSet(s string) map[string]bool {
	out := map[string]bool{}
	for _, w := range strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) }) {
		if len([]rune(w)) >= 4 {
			out[w] = true
		}
	}
	return out
}
