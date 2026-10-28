package host

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A view can be worked, not just read. A row a click opens shows its
// detail and its buttons; each button does what the agent wrote on it:
// sends the agent a message as if you'd typed it, opens a link, copies
// words, or changes the row (ticked, its tone, its word) at once, with no
// turn of the agent's. A checklist's items tick by a click on their mark.
// A change you make is kept with the view, over what the agent says of
// that row, until the agent changes the same itself; the agent hears of
// it at its next step.

// Action is a button: what it says, and what a click on it does, any of
// these at once.
type Action struct {
	Label string  `json:"label"`
	Tone  string  `json:"tone,omitempty"`
	Send  string  `json:"send,omitempty"` // a message to the agent, sent at once
	Open  string  `json:"open,omitempty"` // a web link
	Copy  string  `json:"copy,omitempty"` // words to copy
	Set   *Change `json:"set,omitempty"`  // a change to its row
}

// Change is what a click changes on a row: nil leaves it as it is.
type Change struct {
	Done *bool   `json:"done,omitempty"`
	Tone *string `json:"tone,omitempty"`
	Meta *string `json:"meta,omitempty"`
}

// Yours is a change you made to the row of key, and what the agent had
// there then: when the agent says otherwise, it has changed it itself,
// and its word goes.
type Yours struct {
	Key string `json:"key"`
	Set Change `json:"set"`
	Was Change `json:"was"`
}

const (
	actionsMost = 6    // buttons on a row or a view
	detailMost  = 1200 // characters in a row's detail
	yoursMost   = 200
)

// checkActions are as cut to size: each with a label, its link a web one,
// its change in tones a view has.
func checkActions(as []Action, clip, tone func(*string)) []Action {
	as = as[:min(len(as), actionsMost)]
	out := as[:0]
	for _, a := range as {
		clip(&a.Label)
		if r := []rune(a.Label); len(r) > 24 {
			a.Label = string(r[:23]) + "…"
		}
		tone(&a.Tone)
		clip(&a.Send)
		clip(&a.Copy)
		if !strings.HasPrefix(a.Open, "https://") && !strings.HasPrefix(a.Open, "http://") {
			a.Open = ""
		}
		if c := a.Set; c != nil {
			if c.Tone != nil {
				tone(c.Tone)
			}
			if c.Meta != nil {
				clip(c.Meta)
			}
			if c.Done == nil && c.Tone == nil && c.Meta == nil {
				a.Set = nil
			}
		}
		if a.Label != "" && (a.Send != "" || a.Open != "" || a.Copy != "" || a.Set != nil) {
			out = append(out, a)
		}
	}
	return out
}

// clipDetail is a row's detail cut to size, its lines kept.
func clipDetail(s *string) {
	lines := strings.Split(*s, "\n")
	for i := range lines {
		lines[i] = strings.Join(strings.Fields(lines[i]), " ")
	}
	*s = strings.TrimSpace(strings.Join(lines, "\n"))
	if r := []rune(*s); len(r) > detailMost {
		*s = string(r[:detailMost-1]) + "…"
	}
}

// ItemKey is the key of a row under prefix: its ID, else its text, else
// its first cell.
func ItemKey(prefix, id, text string, cells []string) string {
	switch {
	case id != "":
		return prefix + "/" + id
	case text != "":
		return prefix + "/" + text
	case len(cells) > 0:
		return prefix + "/" + cells[0]
	}
	return ""
}

// BlockKey and GroupKey are the prefixes of a block's rows and a group's.
func BlockKey(bi int) string { return "b" + strconv.Itoa(bi) }
func GroupKey(gi int) string { return "g" + strconv.Itoa(gi) }

// rowRef is a row's fields a change reaches, wherever it is in a view.
type rowRef struct {
	text string
	done **bool // nil for a group's row: it can't be ticked
	tone *string
	meta *string
}

// eachRow calls fn with each of v's rows and its key, until it says stop.
func (v *View) eachRow(fn func(key string, r rowRef) bool) {
	for gi := range v.Groups {
		for i := range v.Groups[gi].Rows {
			r := &v.Groups[gi].Rows[i]
			if k := ItemKey(GroupKey(gi), r.ID, r.Text, nil); k != "" && !fn(k, rowRef{r.Text, nil, &r.Tone, &r.Meta}) {
				return
			}
		}
	}
	var walk func(prefix string, items []Item) bool
	walk = func(prefix string, items []Item) bool {
		for i := range items {
			it := &items[i]
			k := ItemKey(prefix, it.ID, it.Text, it.Cells)
			if k == "" {
				continue
			}
			text := it.Text
			if text == "" && len(it.Cells) > 0 {
				text = it.Cells[0]
			}
			if !fn(k, rowRef{text, &it.Done, &it.Tone, &it.Meta}) || !walk(k, it.Items) {
				return false
			}
		}
		return true
	}
	for bi := range v.Blocks {
		if !walk(BlockKey(bi), v.Blocks[bi].Items) {
			return
		}
	}
}

// row is v's row of key.
func (v *View) row(key string) (rowRef, bool) {
	var got rowRef
	ok := false
	v.eachRow(func(k string, r rowRef) bool {
		if k == key {
			got, ok = r, true
		}
		return !ok
	})
	return got, ok
}

// now is what r has now of the fields c changes.
func (r rowRef) now(c Change) Change {
	var was Change
	if c.Done != nil && r.done != nil {
		d := *r.done != nil && **r.done
		was.Done = &d
	}
	if c.Tone != nil {
		t := *r.tone
		was.Tone = &t
	}
	if c.Meta != nil {
		m := *r.meta
		was.Meta = &m
	}
	return was
}

// apply makes c on r.
func (r rowRef) apply(c Change) {
	if c.Done != nil && r.done != nil {
		d := *c.Done
		*r.done = &d
	}
	if c.Tone != nil {
		*r.tone = *c.Tone
	}
	if c.Meta != nil {
		*r.meta = *c.Meta
	}
}

// keepYours are the changes in ys that still stand on v as the agent has
// it now: each field the agent hasn't changed from what it had when you
// made yours, on a row it still has.
func keepYours(v *View, ys []Yours) []Yours {
	var out []Yours
	for _, y := range ys {
		r, ok := v.row(y.Key)
		if !ok {
			continue
		}
		// What it had reads back as nothing when it was nought or "".
		now := r.now(y.Set)
		if y.Set.Done != nil && (now.Done == nil || *now.Done != deref(y.Was.Done)) {
			y.Set.Done, y.Was.Done = nil, nil
		}
		if y.Set.Tone != nil && *now.Tone != deref(y.Was.Tone) {
			y.Set.Tone, y.Was.Tone = nil, nil
		}
		if y.Set.Meta != nil && *now.Meta != deref(y.Was.Meta) {
			y.Set.Meta, y.Was.Meta = nil, nil
		}
		if y.Set.Done != nil || y.Set.Tone != nil || y.Set.Meta != nil {
			out = append(out, y)
		}
	}
	return out[max(0, len(out)-yoursMost):]
}

func deref[T any](p *T) T {
	var z T
	if p == nil {
		return z
	}
	return *p
}

// applyYours puts your changes on v, to draw it as you left it.
func (v *View) applyYours() {
	for _, y := range v.Yours {
		if r, ok := v.row(y.Key); ok {
			r.apply(y.Set)
		}
	}
}

// ChangeRow makes c on the row of key in the session's view name, as you
// clicked it: kept over the agent's until it changes the same, and told
// the agent at its next step. It's the row's text, for what rush says.
func ChangeRow(id, name, key string, c Change) (string, error) {
	if !viewNameOK.MatchString(name) {
		return "", errors.New("no view " + name)
	}
	p := filepath.Join(viewsDir(id), name+".json")
	b, err := os.ReadFile(p)
	if err != nil {
		return "", err
	}
	var v View
	if err := jsonx.Unmarshal(b, &v); err != nil {
		return "", err
	}
	r, ok := v.row(key)
	if !ok {
		return "", errors.New("that row has gone from the view")
	}
	// What the agent has there: the same row's earlier change knows.
	at := -1
	for i := range v.Yours {
		if v.Yours[i].Key == key {
			at = i
		}
	}
	if at < 0 {
		v.Yours = append(v.Yours, Yours{Key: key})
		at = len(v.Yours) - 1
	}
	y := &v.Yours[at]
	was := r.now(c)
	if c.Done != nil {
		if y.Set.Done == nil {
			y.Was.Done = was.Done
		}
		y.Set.Done = c.Done
	}
	if c.Tone != nil {
		if y.Set.Tone == nil {
			y.Was.Tone = was.Tone
		}
		y.Set.Tone = c.Tone
	}
	if c.Meta != nil {
		if y.Set.Meta == nil {
			y.Was.Meta = was.Meta
		}
		y.Set.Meta = c.Meta
	}
	if c.Done != nil && r.done == nil {
		return "", errors.New("that row can't be ticked")
	}
	b, err = jsonx.Marshal(v)
	if err != nil {
		return "", err
	}
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return "", err
	}
	if err := os.Rename(tmp, p); err != nil {
		return "", err
	}
	_ = Notice(id, changeWords(v.Title, r.text, c))
	return r.text, nil
}

// changeWords tells the agent what you changed on its view.
func changeWords(title, text string, c Change) string {
	var did []string
	if c.Done != nil {
		if *c.Done {
			did = append(did, "ticked it off")
		} else {
			did = append(did, "unticked it")
		}
	}
	if c.Tone != nil {
		if *c.Tone == "" {
			did = append(did, "took its tone off")
		} else {
			did = append(did, "set its tone to "+*c.Tone)
		}
	}
	if c.Meta != nil {
		did = append(did, "set its word to “"+*c.Meta+"”")
	}
	return "In your view “" + title + "”, the user clicked “" + text + "” and " + strings.Join(did, ", ") +
		". The view keeps it; set the row the same when you next update the view, or say otherwise."
}
