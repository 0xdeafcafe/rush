package host

import (
	"errors"
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
const viewsPrompt = `When your work tracks many things over time (pull requests, failing tests, services, a migration's files), keep them in a view rush draws beside the conversation, instead of repeating the list in each message: pipe JSON to ` + "`rush view set <name>`" + ` and set it again as things change; ` + "`rush view help`" + ` shows the shape. Its summary is your line in rush's list.`

// View is one panel: a title, a few numbers, and rows in groups.
type View struct {
	Name    string    `json:"name"`
	Title   string    `json:"title"`
	Summary string    `json:"summary,omitempty"` // one line, under the session in the list
	Stats   []Stat    `json:"stats,omitempty"`
	Groups  []Group   `json:"groups,omitempty"`
	Note    string    `json:"note,omitempty"` // a quiet line at the end
	Updated time.Time `json:"updated"`
}

// Stat is a number along a view's top: "8" "ready".
type Stat struct {
	Value string `json:"value"`
	Label string `json:"label"`
	Tone  string `json:"tone,omitempty"`
}

// Group is a titled run of rows.
type Group struct {
	Title string `json:"title,omitempty"`
	Rows  []Row  `json:"rows"`
}

// Row is one line of a view: what it is, a quiet word on it, and a link.
// Tone colours its mark: good, bad, warn, busy or quiet.
type Row struct {
	Text string `json:"text"`
	Meta string `json:"meta,omitempty"`
	Tone string `json:"tone,omitempty"`
	Link string `json:"link,omitempty"`
}

// Tones a view may use; anything else reads as plain.
var Tones = []string{"good", "bad", "warn", "busy", "quiet"}

const (
	viewsMost    = 6   // views a session may keep
	viewRowsMost = 200 // rows across a view's groups
	viewTextMost = 300 // characters in any one text
	viewName     = `^[a-z0-9][a-z0-9-]{0,31}$`
)

var viewNameOK = regexp.MustCompile(viewName)

func viewsDir(id string) string { return filepath.Join(dir(id), "views") }

// SaveView keeps v as the session's view of its name, in place of any
// before it, checked and cut to size.
func SaveView(id string, v View) error {
	if !viewNameOK.MatchString(v.Name) {
		return errors.New("a view's name is lowercase letters, digits and dashes, up to 32: like prs or tests")
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
	if es, _ := os.ReadDir(d); len(es) >= viewsMost && !slices.ContainsFunc(es, func(e os.DirEntry) bool { return e.Name() == v.Name+".json" }) {
		return errors.New("a session keeps at most 6 views: clear one first (rush view clear <name>)")
	}
	v.Updated = time.Now()
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
	if v.Title == "" {
		return errors.New(`a view needs a title: {"title": "Pull requests", …}`)
	}
	v.Stats = v.Stats[:min(len(v.Stats), 4)]
	for i := range v.Stats {
		clip(&v.Stats[i].Value)
		clip(&v.Stats[i].Label)
		tone(&v.Stats[i].Tone)
	}
	left := viewRowsMost
	for g := range v.Groups {
		clip(&v.Groups[g].Title)
		rows := v.Groups[g].Rows[:min(len(v.Groups[g].Rows), left)]
		for i := range rows {
			clip(&rows[i].Text)
			clip(&rows[i].Meta)
			tone(&rows[i].Tone)
			if !strings.HasPrefix(rows[i].Link, "https://") && !strings.HasPrefix(rows[i].Link, "http://") {
				rows[i].Link = "" // only web links: a click opens it
			}
		}
		v.Groups[g].Rows, left = rows, left-len(rows)
	}
	return nil
}

// DropView takes the session's view of name away; none is no error.
func DropView(id, name string) error {
	err := os.Remove(filepath.Join(viewsDir(id), name+".json"))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
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
