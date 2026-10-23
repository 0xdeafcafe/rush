package host

import (
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// An agent can keep views of its own work: a panel rush draws beside its
// conversation, in rush's own parts, and a line for the list. The agent
// says what to show (`rush view set prs`, JSON on stdin) and rush draws
// it; the same name again updates it in place. Plugins and your scripts
// show one the same way. Each lives in the session's folder, one file a
// view, so any rush that has the session open reads it.

// viewsPrompt tells every agent it can keep a view: kept short, as every
// session carries it.
const viewsPrompt = `When your work tracks many things over time (a list you were handed to work through, a long search across the code, pull requests, failing tests, services, a migration's modules), keep them in a view rush draws beside the conversation, instead of repeating the list in each message: pipe JSON to ` + "`rush view set <name>`" + ` and set it again as things change; ` + "`rush view guide`" + ` says what makes a good one (read it before your first), ` + "`rush view help`" + ` the shape and ` + "`rush view example`" + ` ready ones (charts, tiles, trends). Its summary is your line in rush's list.`

// View is one panel: a title, a few numbers, and rows in groups.
type View struct {
	Name    string `json:"name"`
	Title   string `json:"title"`
	Summary string `json:"summary,omitempty"` // one line, under the session in the list
	Tone    string `json:"tone,omitempty"`    // its title's colour
	// Progress is a bar under the title: done of total, and a word on it.
	Progress *Progress `json:"progress,omitempty"`
	Stats    []Stat    `json:"stats,omitempty"`
	Groups   []Group   `json:"groups,omitempty"`
	Note     string    `json:"note,omitempty"` // a quiet line at the end
	// Blocks are the view's parts after those above, each in a style of
	// its own: a table, a chart, a timeline… (BlockKinds).
	Blocks []Block `json:"blocks,omitempty"`
	// Accent is the view's own colour, for its mark and what has no tone:
	// one of Accents; "" is rush's orange.
	Accent string `json:"accent,omitempty"`
	// By is the plugin that drew it; "" is the session's agent.
	By string `json:"by,omitempty"`
	// Pinned is a view you brought back from the past ones: rush doesn't
	// put it away by itself.
	Pinned  bool      `json:"pinned,omitempty"`
	Updated time.Time `json:"updated"`
	// Gone is when it was put away, for a past view; File is its file.
	Gone time.Time `json:"gone,omitzero"`
	File string    `json:"-"`
}

// Block is a part of a view drawn in one style, its Kind; which of its
// fields it reads depends on that.
//
//	table      columns, items[].cells (+ tone, link)
//	kv         items[]: text is the key, meta the value
//	checklist  items[]: text, done, meta
//	log        items[]: time, text, tone
//	list       items[]: text, meta, tone, link, progress (a group's rows)
//	bars       items[]: text, value, meta
//	chart      values, labels; style "columns" or "line"
//	heatmap    columns; items[]: text, values
//	compare    items[]: text, from, to, tone
//	steps      items[]: text, meta, tone (a pipeline, left to right)
//	timeline   items[]: time, text, meta, tone
//	board      items[] are columns: text, tone, items[] its cards
//	tree       items[]: text, meta, tone, items[] its children
//	hero       value, text, tone, trend
//	callout    text, tone
//	code       text, as it is, lines and all
//	text       text
//	stats      stats
//	progress   progress
type Block struct {
	Kind     string    `json:"kind"`
	Title    string    `json:"title,omitempty"`
	Tone     string    `json:"tone,omitempty"`
	Folded   bool      `json:"folded,omitempty"`
	Text     string    `json:"text,omitempty"`
	Value    string    `json:"value,omitempty"` // a hero's number
	Style    string    `json:"style,omitempty"` // a chart's: columns or line
	Columns  []string  `json:"columns,omitempty"`
	Labels   []string  `json:"labels,omitempty"`
	Values   []float64 `json:"values,omitempty"`
	Trend    []float64 `json:"trend,omitempty"`
	Items    []Item    `json:"items,omitempty"`
	Stats    []Stat    `json:"stats,omitempty"`
	Progress *Progress `json:"progress,omitempty"`
}

// Item is one thing in a block: a row, a cell's row, a card, an event, a
// step, a node.
type Item struct {
	Text     string    `json:"text,omitempty"`
	Meta     string    `json:"meta,omitempty"`
	Tone     string    `json:"tone,omitempty"`
	Link     string    `json:"link,omitempty"`
	Time     string    `json:"time,omitempty"`
	Value    *float64  `json:"value,omitempty"`
	Progress *float64  `json:"progress,omitempty"`
	Done     *bool     `json:"done,omitempty"`
	From     string    `json:"from,omitempty"`
	To       string    `json:"to,omitempty"`
	Cells    []string  `json:"cells,omitempty"`
	Values   []float64 `json:"values,omitempty"`
	Items    []Item    `json:"items,omitempty"`
}

// BlockKinds are the styles a block may be drawn in.
var BlockKinds = []string{"table", "kv", "checklist", "log", "list", "bars", "chart", "heatmap", "compare",
	"steps", "timeline", "board", "tree", "hero", "callout", "code", "text", "stats", "progress"}

// Accents are the colours a view may take as its own.
var Accents = []string{"orange", "blue", "green", "yellow", "purple", "red"}

// Stat is a number along a view's top: "8" "ready".
type Stat struct {
	Value string `json:"value"`
	Label string `json:"label"`
	Tone  string `json:"tone,omitempty"`
	// Trend is how the number has gone, oldest first: a sparkline under it.
	Trend []float64 `json:"trend,omitempty"`
}

// Progress is how far along something is: done of total.
type Progress struct {
	Done  float64 `json:"done"`
	Total float64 `json:"total"`
	Label string  `json:"label,omitempty"`
	Tone  string  `json:"tone,omitempty"`
}

// Group is a titled run of rows; its tone colours its title. Text is a
// few words under its title; Folded starts it closed, a click opens it.
type Group struct {
	Title  string `json:"title,omitempty"`
	Tone   string `json:"tone,omitempty"`
	Text   string `json:"text,omitempty"`
	Folded bool   `json:"folded,omitempty"`
	Rows   []Row  `json:"rows"`
}

// Row is one line of a view: what it is, a quiet word on it, and a link.
// Tone colours its mark: good, bad, warn, busy or quiet.
type Row struct {
	Text string `json:"text"`
	Meta string `json:"meta,omitempty"`
	Tone string `json:"tone,omitempty"`
	Link string `json:"link,omitempty"`
	// Progress, 0 to 1, draws a small bar at the row's end.
	Progress *float64 `json:"progress,omitempty"`
	// Value is a number drawn as a bar against the group's largest, a
	// chart: its meta, if any, is the number's words ("212ms").
	Value *float64 `json:"value,omitempty"`
}

// Tones a view may use; anything else reads as plain.
var Tones = []string{"good", "bad", "warn", "busy", "quiet"}

const (
	pastMost     = 40  // past views a session keeps
	blocksMost   = 16  // blocks in a view
	itemsMost    = 200 // items in a block, all its levels
	cellsMost    = 8   // a table's columns, a heatmap's are 40
	valuesMost   = 120 // a chart's or heatmap row's numbers
	codeMost     = 60  // a code block's lines
	viewsMost    = 6   // views a session may keep
	viewRowsMost = 200 // rows across a view's groups
	viewTextMost = 300 // characters in any one text
	viewName     = `^[a-z0-9][a-z0-9-]{0,63}$`
)

var viewNameOK = regexp.MustCompile(viewName)

func viewsDir(id string) string { return filepath.Join(dir(id), "views") }

// SaveView keeps v as the session's view of its name, in place of any
// before it, checked and cut to size.
func SaveView(id string, v View) error {
	if !viewNameOK.MatchString(v.Name) {
		return errors.New("a view's name is lowercase letters, digits and dashes, up to 64: like prs or tests")
	}
	if _, err := os.Stat(dir(id)); err != nil {
		return errors.New("no rush session " + id)
	}
	if err := v.check(); err != nil {
		return err
	}
	d := viewsDir(id)
	if err := os.MkdirAll(d, 0o700); err != nil {
		return err
	}
	es, _ := os.ReadDir(d)
	es = slices.DeleteFunc(es, func(e os.DirEntry) bool { return !strings.HasSuffix(e.Name(), ".json") })
	if len(es) >= viewsMost && !slices.ContainsFunc(es, func(e os.DirEntry) bool { return e.Name() == v.Name+".json" }) {
		return errors.New("a session keeps at most 6 views: clear one first (rush view clear <name>)")
	}
	// One you brought back stays yours to put away.
	if b, err := os.ReadFile(filepath.Join(d, v.Name+".json")); err == nil {
		var was View
		if jsonx.Unmarshal(b, &was) == nil {
			v.Pinned = v.Pinned || was.Pinned
		}
	}
	v.Updated, v.Gone = time.Now(), time.Time{}
	b, err := jsonx.Marshal(v)
	if err != nil {
		return err
	}
	tmp := filepath.Join(d, v.Name+".json.tmp")
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, filepath.Join(d, v.Name+".json"))
}

// check is v cut to what a panel can show, or why it can't be.
func (v *View) check() error {
	clip := func(s *string) {
		*s = strings.Join(strings.Fields(*s), " ") // one line each
		if r := []rune(*s); len(r) > viewTextMost {
			*s = string(r[:viewTextMost-1]) + "…"
		}
	}
	tone := func(t *string) {
		if !slices.Contains(Tones, *t) {
			*t = ""
		}
	}
	clip(&v.Title)
	clip(&v.Summary)
	clip(&v.Note)
	tone(&v.Tone)
	if !slices.Contains(Accents, v.Accent) {
		v.Accent = ""
	}
	v.Blocks = v.Blocks[:min(len(v.Blocks), blocksMost)]
	for i := range v.Blocks {
		if err := v.Blocks[i].check(clip, tone); err != nil {
			return fmt.Errorf("block %d: %w", i+1, err)
		}
	}
	if p := v.Progress; p != nil {
		clip(&p.Label)
		tone(&p.Tone)
		if p.Total <= 0 {
			v.Progress = nil
		}
	}
	if v.Title == "" {
		return errors.New(`a view needs a title: {"title": "Pull requests", …}`)
	}
	v.Stats = v.Stats[:min(len(v.Stats), 4)]
	for i := range v.Stats {
		clip(&v.Stats[i].Value)
		clip(&v.Stats[i].Label)
		tone(&v.Stats[i].Tone)
		t := v.Stats[i].Trend
		v.Stats[i].Trend = t[max(0, len(t)-40):]
	}
	left := viewRowsMost
	for g := range v.Groups {
		clip(&v.Groups[g].Title)
		clip(&v.Groups[g].Text)
		tone(&v.Groups[g].Tone)
		rows := v.Groups[g].Rows[:min(len(v.Groups[g].Rows), left)]
		for i := range rows {
			clip(&rows[i].Text)
			clip(&rows[i].Meta)
			tone(&rows[i].Tone)
			if p := rows[i].Progress; p != nil {
				*p = min(1, max(0, *p))
			}
			if p := rows[i].Value; p != nil && (*p < 0 || math.IsNaN(*p) || math.IsInf(*p, 0)) {
				rows[i].Value = nil // a bar is a size: nought or more
			}
			if !strings.HasPrefix(rows[i].Link, "https://") && !strings.HasPrefix(rows[i].Link, "http://") {
				rows[i].Link = "" // only web links: a click opens it
			}
		}
		v.Groups[g].Rows, left = rows, left-len(rows)
	}
	return nil
}

// check is b cut to what a panel can show, or why it can't be.
func (b *Block) check(clip, tone func(*string)) error {
	if !slices.Contains(BlockKinds, b.Kind) {
		return fmt.Errorf("kind %q: use one of %s", b.Kind, strings.Join(BlockKinds, ", "))
	}
	clip(&b.Title)
	tone(&b.Tone)
	clip(&b.Value)
	if b.Style != "line" {
		b.Style = ""
	}
	if b.Kind == "code" {
		lines := strings.Split(strings.ReplaceAll(b.Text, "\t", "    "), "\n")
		lines = lines[:min(len(lines), codeMost)]
		for i := range lines {
			lines[i] = strings.Map(func(r rune) rune {
				if r < 0x20 || r == 0x7f {
					return -1
				}
				return r
			}, lines[i])
			if r := []rune(lines[i]); len(r) > viewTextMost {
				lines[i] = string(r[:viewTextMost])
			}
		}
		b.Text = strings.Join(lines, "\n")
	} else {
		clip(&b.Text)
	}
	b.Columns = b.Columns[:min(len(b.Columns), 40)]
	if b.Kind == "table" {
		b.Columns = b.Columns[:min(len(b.Columns), cellsMost)]
	}
	for i := range b.Columns {
		clip(&b.Columns[i])
	}
	b.Labels = b.Labels[:min(len(b.Labels), valuesMost)]
	for i := range b.Labels {
		clip(&b.Labels[i])
	}
	b.Values = finite(b.Values[max(0, len(b.Values)-valuesMost):])
	b.Trend = finite(b.Trend[max(0, len(b.Trend)-40):])
	b.Stats = b.Stats[:min(len(b.Stats), 4)]
	for i := range b.Stats {
		clip(&b.Stats[i].Value)
		clip(&b.Stats[i].Label)
		tone(&b.Stats[i].Tone)
		b.Stats[i].Trend = finite(b.Stats[i].Trend[max(0, len(b.Stats[i].Trend)-40):])
	}
	if p := b.Progress; p != nil {
		clip(&p.Label)
		tone(&p.Tone)
		if p.Total <= 0 {
			b.Progress = nil
		}
	}
	left := itemsMost
	b.Items = checkItems(b.Items, 0, &left, clip, tone)
	return nil
}

// checkItems are items cut to size, down to 4 levels and left items in
// all, each clipped and its link a web one.
func checkItems(items []Item, depth int, left *int, clip, tone func(*string)) []Item {
	if depth >= 4 || *left <= 0 {
		return nil
	}
	items = items[:min(len(items), *left)]
	*left -= len(items)
	for i := range items {
		it := &items[i]
		clip(&it.Text)
		clip(&it.Meta)
		clip(&it.Time)
		clip(&it.From)
		clip(&it.To)
		tone(&it.Tone)
		if !strings.HasPrefix(it.Link, "https://") && !strings.HasPrefix(it.Link, "http://") {
			it.Link = ""
		}
		if p := it.Progress; p != nil {
			*p = min(1, max(0, *p))
		}
		if p := it.Value; p != nil && (*p < 0 || math.IsNaN(*p) || math.IsInf(*p, 0)) {
			it.Value = nil
		}
		it.Cells = it.Cells[:min(len(it.Cells), 40)]
		for c := range it.Cells {
			clip(&it.Cells[c])
		}
		it.Values = finite(it.Values[:min(len(it.Values), valuesMost)])
		it.Items = checkItems(it.Items, depth+1, left, clip, tone)
	}
	return items
}

// finite is vs with any number that isn't one made nought.
func finite(vs []float64) []float64 {
	for i, v := range vs {
		if math.IsNaN(v) || math.IsInf(v, 0) {
			vs[i] = 0
		}
	}
	return vs
}

// DropView puts the session's view of name away among its past ones,
// where you can bring it back; none is no error.
func DropView(id, name string) error {
	from := filepath.Join(viewsDir(id), name+".json")
	b, err := os.ReadFile(from)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var v View
	if jsonx.Unmarshal(b, &v) == nil && v.Title != "" {
		v.Gone, v.Pinned = time.Now(), false
		past := filepath.Join(viewsDir(id), "past")
		if err := os.MkdirAll(past, 0o700); err != nil {
			return err
		}
		if b, err := jsonx.Marshal(v); err == nil {
			_ = os.WriteFile(filepath.Join(past, fmt.Sprintf("%d-%s.json", v.Gone.UnixNano(), name)), b, 0o600)
		}
		prunePast(past)
	}
	return os.Remove(from)
}

// prunePast keeps the newest pastMost past views.
func prunePast(past string) {
	es, _ := os.ReadDir(past)
	if len(es) <= pastMost {
		return
	}
	// Named by when they went, so their names sort oldest first.
	slices.SortFunc(es, func(a, b os.DirEntry) int { return strings.Compare(pastKey(a.Name()), pastKey(b.Name())) })
	for _, e := range es[:len(es)-pastMost] {
		_ = os.Remove(filepath.Join(past, e.Name()))
	}
}

// pastKey is a past view's file name with its time padded, to sort by.
func pastKey(name string) string {
	t, rest, _ := strings.Cut(name, "-")
	return fmt.Sprintf("%020s-%s", t, rest)
}

// PastViews are the session's views put away, the one gone last first.
func PastViews(id string) []View {
	past := filepath.Join(viewsDir(id), "past")
	es, _ := os.ReadDir(past)
	var vs []View
	for _, e := range es {
		b, err := os.ReadFile(filepath.Join(past, e.Name()))
		if err != nil {
			continue
		}
		var v View
		if jsonx.Unmarshal(b, &v) == nil && v.Title != "" {
			v.File = e.Name()
			vs = append(vs, v)
		}
	}
	slices.SortFunc(vs, func(a, b View) int { return b.Gone.Compare(a.Gone) })
	return vs
}

// BringBack shows a past view again, pinned: rush won't put it away by
// itself. It takes the place of a view of its name showing now, which
// goes among the past ones.
func BringBack(id, file string) error {
	if file != filepath.Base(file) || !strings.HasSuffix(file, ".json") {
		return errors.New("no past view " + file)
	}
	from := filepath.Join(viewsDir(id), "past", file)
	b, err := os.ReadFile(from)
	if err != nil {
		return err
	}
	var v View
	if err := jsonx.Unmarshal(b, &v); err != nil {
		return err
	}
	if err := DropView(id, v.Name); err != nil {
		return err
	}
	v.Pinned = true
	if err := SaveView(id, v); err != nil {
		return err
	}
	return os.Remove(from)
}

// PutAwayStale puts away the session's views the agent hasn't touched
// since since: the start of its third turn back. Pinned ones stay.
func PutAwayStale(id string, since time.Time) {
	vs, _ := Views(id)
	for _, v := range vs {
		if !v.Pinned && v.By == "" && v.Updated.Before(since) {
			_ = DropView(id, v.Name)
		}
	}
}

// Views are the session's views, the one updated last first, and when
// its folder last changed, to read them again only then.
func Views(id string) (vs []View, at time.Time) {
	d := viewsDir(id)
	st, err := os.Stat(d)
	if err != nil {
		return nil, time.Time{}
	}
	es, _ := os.ReadDir(d)
	for _, e := range es {
		if !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(d, e.Name()))
		if err != nil {
			continue
		}
		var v View
		if jsonx.Unmarshal(b, &v) == nil && v.Title != "" {
			vs = append(vs, v)
		}
	}
	slices.SortFunc(vs, func(a, b View) int { return b.Updated.Compare(a.Updated) })
	return vs, st.ModTime()
}

// ViewsChanged is when the session's views last changed: a view saved or
// cleared renames a file in its folder, which the folder's time shows.
func ViewsChanged(id string) time.Time {
	st, err := os.Stat(viewsDir(id))
	if err != nil {
		return time.Time{}
	}
	return st.ModTime()
}
