package main

import (
	"bufio"
	"fmt"
	"github.com/0xdeafcafe/photon/jsonx"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
)

// card is the part of a kanban-code card (a "link" in ~/.kanban-code/
// links.json) this plugin reads. kanban-code's app owns the file; the
// plugin only reads it, and changes cards through the kanban CLI.
type card struct {
	ID               string `json:"id"`
	Name             string `json:"name"`
	ProjectPath      string `json:"projectPath"`
	Column           string `json:"column"`
	ManuallyArchived bool   `json:"manuallyArchived"`
	ParentCardID     string `json:"parentCardId"`
	PromptBody       string `json:"promptBody"`
	LastActivity     string `json:"lastActivity"`
	SortOrder        *int   `json:"sortOrder"`
	UpdatedAt        string `json:"updatedAt"`
	SessionLink      *struct {
		SessionID string `json:"sessionId"`
	} `json:"sessionLink"`
	WorktreeLink *struct {
		Path   string `json:"path"`
		Branch string `json:"branch"`
	} `json:"worktreeLink"`
	IssueLink *struct {
		Number int    `json:"number"`
		URL    string `json:"url"`
		Title  string `json:"title"`
		Body   string `json:"body"`
	} `json:"issueLink"`
	PRLinks       []pr `json:"prLinks"`
	QueuedPrompts []struct {
		Body string `json:"body"`
	} `json:"queuedPrompts"`
}

type pr struct {
	Number                   int    `json:"number"`
	URL                      string `json:"url"`
	Title                    string `json:"title"`
	Status                   string `json:"status"`
	UnresolvedThreads        int    `json:"unresolvedThreads"`
	FirstUnresolvedThreadURL string `json:"firstUnresolvedThreadURL"`
	CheckRuns                []struct {
		Name       string `json:"name"`
		Status     string `json:"status"`
		Conclusion string `json:"conclusion"`
	} `json:"checkRuns"`
}

// failing are the names of its checks that finished badly.
func (p pr) failing() []string {
	var out []string
	for _, c := range p.CheckRuns {
		switch c.Conclusion {
		case "failure", "timed_out", "cancelled", "action_required":
			out = append(out, c.Name)
		}
	}
	sort.Strings(out)
	return slices.Compact(out)
}

// columns in board order, as kanban-code names and shows them.
var columns = []struct{ id, title string }{
	{"in_progress", "In Progress"},
	{"requires_attention", "Waiting"},
	{"in_review", "In Review"},
	{"backlog", "Backlog"},
	{"done", "Done"},
}

func columnTitle(id string) string {
	for _, c := range columns {
		if c.id == id {
			return c.title
		}
	}
	return id
}

func kanbanHome() string {
	if h := os.Getenv("KANBAN_CODE_HOME"); h != "" {
		return h
	}
	return filepath.Join(os.Getenv("HOME"), ".kanban-code")
}

// readCards reads the board: every card that's on it, not archived, and
// not a subagent of another.
func readCards() ([]card, error) {
	f, err := os.Open(filepath.Join(kanbanHome(), "links.json"))
	if err != nil {
		if os.IsNotExist(err) {
			return nil, fmt.Errorf("no kanban-code board at %s: is kanban-code installed, and KANBAN_CODE_HOME right in plugin.json?", kanbanHome())
		}
		return nil, err
	}
	defer f.Close()
	// A card at a time: the file holds every conversation kanban-code has
	// seen, and can be tens of megabytes, most of it cards left off here.
	dec := jsonx.NewDecoder(bufio.NewReaderSize(f, 64<<10))
	var out []card
	fail := func(err error) ([]card, error) { return nil, fmt.Errorf("reading links.json: %w", err) }
	if _, err := dec.ReadToken(); err != nil { // {
		return fail(err)
	}
	for dec.PeekKind() != '}' {
		key, err := dec.ReadToken()
		if err != nil {
			return fail(err)
		}
		if key.String() != "links" {
			if err := dec.SkipValue(); err != nil {
				return fail(err)
			}
			continue
		}
		if _, err := dec.ReadToken(); err != nil { // [
			return fail(err)
		}
		for dec.PeekKind() != ']' {
			var c card
			if err := jsonx.DecodeValue(dec, &c); err != nil {
				return fail(err)
			}
			if !c.ManuallyArchived && c.ParentCardID == "" && c.Column != "all_sessions" {
				out = append(out, c)
			}
		}
		if _, err := dec.ReadToken(); err != nil { // ]
			return fail(err)
		}
	}
	return out, nil
}

func (c card) title() string {
	switch {
	case c.Name != "":
		return c.Name
	case c.IssueLink != nil && c.IssueLink.Title != "":
		return c.IssueLink.Title
	case c.PromptBody != "":
		return firstLine(c.PromptBody)
	}
	return c.ID
}

func (c card) sessionID() string {
	if c.SessionLink == nil {
		return ""
	}
	return c.SessionLink.SessionID
}

// findCard is a card by id, id prefix, or name, as the kanban CLI finds
// them.
func findCard(cards []card, q string) (card, error) {
	q = strings.TrimSpace(q)
	var byPrefix []card
	for _, c := range cards {
		if c.ID == q {
			return c, nil
		}
		if strings.HasPrefix(c.ID, q) {
			byPrefix = append(byPrefix, c)
		}
	}
	if len(byPrefix) == 1 {
		return byPrefix[0], nil
	}
	for _, c := range cards {
		if strings.EqualFold(c.title(), q) {
			return c, nil
		}
	}
	return card{}, fmt.Errorf("no card %q on the board", q)
}

// cardOf is the card a session works on: the one it was started for, the
// one kanban-code linked to its conversation, or the one whose worktree it
// is in.
func cardOf(cards []card, s session) (card, bool) {
	if id := s.Meta["card"]; id != "" {
		for _, c := range cards {
			if c.ID == id {
				return c, true
			}
		}
	}
	for _, c := range cards {
		if s.SessionID != "" && c.sessionID() == s.SessionID {
			return c, true
		}
	}
	for _, c := range cards {
		if w := c.WorktreeLink; w != nil && w.Path != "" && within(s.Cwd, w.Path) {
			return c, true
		}
	}
	return card{}, false
}

func within(dir, root string) bool {
	root = strings.TrimSuffix(root, "/")
	return dir == root || strings.HasPrefix(dir, root+"/")
}

var slugUnsafe = regexp.MustCompile(`[^a-z0-9]+`)

// slug is a branch and folder name made from a card's title.
func slug(s string) string {
	s = strings.Trim(slugUnsafe.ReplaceAllString(strings.ToLower(s), "-"), "-")
	if len(s) > 40 {
		s = strings.TrimRight(s[:40], "-")
	}
	if s == "" {
		return "card"
	}
	return s
}

func firstLine(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	if len(s) > 80 {
		s = s[:80] + "…"
	}
	return s
}

// describe is a card in full, for the agent working on it.
func describe(c card) string {
	var b strings.Builder
	w := func(f string, a ...any) { fmt.Fprintf(&b, f, a...) }
	w("Card %s: %s\nColumn: %s\n", c.ID, c.title(), columnTitle(c.Column))
	if c.ProjectPath != "" {
		w("Project: %s\n", c.ProjectPath)
	}
	if wt := c.WorktreeLink; wt != nil {
		w("Worktree: %s (branch %s)\n", wt.Path, wt.Branch)
	}
	if is := c.IssueLink; is != nil {
		w("\nIssue #%d: %s\n%s\n", is.Number, is.Title, is.URL)
		if body := strings.TrimSpace(is.Body); body != "" {
			w("\n%s\n", body)
		}
	}
	if p := strings.TrimSpace(c.PromptBody); p != "" {
		w("\nTask:\n%s\n", p)
	}
	for _, p := range c.PRLinks {
		w("\nPR #%d", p.Number)
		if p.Title != "" {
			w(": %s", p.Title)
		}
		w("\n")
		if p.URL != "" {
			w("%s\n", p.URL)
		}
		if p.Status != "" {
			w("Status: %s\n", p.Status)
		}
		if f := p.failing(); len(f) > 0 {
			w("Failing checks: %s\n", strings.Join(f, ", "))
		}
		if p.UnresolvedThreads > 0 {
			w("Unresolved review threads: %d (first: %s)\n", p.UnresolvedThreads, p.FirstUnresolvedThreadURL)
		}
	}
	if len(c.QueuedPrompts) > 0 {
		w("\nQueued for later:\n")
		for _, q := range c.QueuedPrompts {
			w("- %s\n", firstLine(q.Body))
		}
	}
	return b.String()
}

// board is the board, a column at a time.
func board(cards []card, column string) string {
	var b strings.Builder
	for _, col := range columns {
		if column != "" && col.id != column {
			continue
		}
		var in []card
		for _, c := range cards {
			if c.Column == col.id {
				in = append(in, c)
			}
		}
		if len(in) == 0 {
			continue
		}
		sortColumn(in)
		fmt.Fprintf(&b, "## %s\n", col.title)
		for _, c := range in {
			fmt.Fprintf(&b, "- %s  %s", c.ID, c.title())
			if wt := c.WorktreeLink; wt != nil && wt.Branch != "" {
				fmt.Fprintf(&b, "  [%s]", wt.Branch)
			}
			for _, p := range c.PRLinks {
				fmt.Fprintf(&b, "  PR #%d %s", p.Number, p.Status)
			}
			b.WriteString("\n")
		}
		b.WriteString("\n")
	}
	if b.Len() == 0 {
		return "The board is empty."
	}
	return strings.TrimSpace(b.String())
}

// sortColumn puts a column's cards in the order kanban-code shows them:
// those given a place first, by it, then the most recently active.
func sortColumn(cards []card) {
	sort.SliceStable(cards, func(i, j int) bool {
		a, b := cards[i], cards[j]
		switch {
		case a.SortOrder != nil && b.SortOrder != nil:
			return *a.SortOrder < *b.SortOrder
		case a.SortOrder != nil || b.SortOrder != nil:
			return a.SortOrder != nil
		}
		if ta, tb := or(a.LastActivity, a.UpdatedAt), or(b.LastActivity, b.UpdatedAt); ta != tb {
			return ta > tb
		}
		return a.ID < b.ID
	})
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}
