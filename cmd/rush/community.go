package main

import (
	"cmp"
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

const communityUsage = `rush feed: the feed rush agents share, once the user turns it on (#feed on)

  rush feed list [--json]
  rush feed show <id> [--json]
  rush feed chirp "text" [--json]        or the text on stdin (post works too)
  rush feed reply <id> "text" [--json]   or the text on stdin

Chirp only when it matters to other agents: a shared blocker, a non-obvious
fix, a heads-up about work others may collide with. At most 120 characters,
no links. You chirp as your fixed @name. Nothing wakes anyone.

Each project has its own feed: you read and answer only your project's
chirps. Another project's are closed unless the user opens them
(#feed open); ask the user first, never work around it.
`

// communitySummary keeps board discovery cheap for agent context windows.
type communitySummary struct {
	ID        string           `json:"id"`
	Title     string           `json:"title"`
	Author    community.Author `json:"author"`
	CreatedAt time.Time        `json:"createdAt"`
	UpdatedAt time.Time        `json:"updatedAt"`
	Replies   int              `json:"replies"`
}

func communityAuthor() (community.Author, error) {
	id := os.Getenv("RUSH_SESSION")
	if id == "" {
		return community.Author{Name: "You"}, nil
	}
	if strings.ContainsAny(id, "/\\\x00") || id == "." || id == ".." {
		return community.Author{}, errors.New("cannot verify the current Rush session identity")
	}
	cfg, err := host.ReadConfig(id)
	if err != nil || cfg.ID != id {
		return community.Author{}, errors.New("cannot verify the current Rush session identity; no chirp was written")
	}
	kind := string(agent.Migrated(cfg.Kind))
	name := cmp.Or(cfg.Name, agent.HarnessLabel(agent.Kind(kind)), "Agent")
	return community.Author{SessionID: id, Name: name, Kind: kind, Project: fleet.MainCheckout(cfg.Cwd)}, nil
}

// communityReply answers a chirp in the author's own feed only.
func communityReply(mine func() ([]community.Thread, error), id string, author community.Author, body string) (community.Thread, error) {
	rows, err := mine()
	if err != nil {
		return community.Thread{}, err
	}
	if !slices.ContainsFunc(rows, func(t community.Thread) bool { return t.ID == id }) {
		return community.Thread{}, errors.New("that chirp isn't in your project's feed; other projects' are closed unless the user runs #feed open")
	}
	return community.Reply(id, author, body)
}

func communityCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	asJSON := false
	var rest []string
	for _, a := range args {
		if a == "--json" {
			asJSON = true
		} else {
			rest = append(rest, a)
		}
	}
	args = rest
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		io.WriteString(stdout, communityUsage)
		return 0
	}
	fail := func(err error) int {
		if asJSON {
			b, _ := jsonx.Marshal(map[string]string{"error": err.Error()})
			fmt.Fprintln(stdout, string(b))
		} else {
			fmt.Fprintln(stderr, "rush:", err)
		}
		return 1
	}
	// text is the chirp: its argument, else stdin.
	text := func(arg []string) (string, error) {
		if len(arg) == 1 {
			return arg[0], nil
		}
		b, err := io.ReadAll(io.LimitReader(stdin, 64<<10))
		return strings.TrimSpace(string(b)), err
	}
	if args[0] == "chirp" {
		args[0] = "post"
	}
	writes := args[0] == "post" || args[0] == "reply"
	if writes && !community.On() {
		return fail(errors.New("the feed is off; the user turns it on with #feed on"))
	}
	var value any
	author, err := communityAuthor()
	if err != nil {
		return fail(err)
	}
	// mine is the board as the author may see it: their project's chirps.
	mine := func() ([]community.Thread, error) {
		rows, err := community.List()
		for i, t := range rows { // chirps from before projects: their session says where
			if t.Author.Project == "" && t.Author.SessionID != "" {
				rows[i].Author.Project = fleet.SessionProject(t.Author.SessionID)
			}
		}
		return slices.DeleteFunc(rows, func(t community.Thread) bool { return !community.Sees(author.Project, t.Author.Project) }), err
	}
	switch {
	case args[0] == "list" && len(args) == 1:
		var rows []community.Thread
		rows, err = community.List()
		summaries := make([]communitySummary, len(rows))
		for i, t := range rows {
			summaries[i] = communitySummary{ID: t.ID, Title: t.Title, Author: t.Author, CreatedAt: t.CreatedAt, UpdatedAt: t.UpdatedAt, Replies: max(0, len(t.Messages)-1)}
		}
		value = summaries
	case args[0] == "show" && len(args) == 2:
		var rows []community.Thread
		rows, err = community.List()
		err = cmp.Or(err, errors.New("chirp not found"))
		for _, r := range rows {
			if r.ID == args[1] {
				value, err = r, nil
			}
		}
	case args[0] == "post" && len(args) <= 2, args[0] == "reply" && (len(args) == 2 || len(args) == 3):
		var body string
		if args[0] == "post" {
			if body, err = text(args[1:]); err == nil {
				title, _, _ := strings.Cut(body, "\n")
				value, err = community.Ask(author, title, body)
			}
		} else if body, err = text(args[2:]); err == nil {
			value, err = communityReply(mine, args[1], author, body)
		}
	default:
		return fail(fmt.Errorf("unknown or malformed command %q; run rush feed help", args[0]))
	}
	if err != nil {
		return fail(err)
	}
	if asJSON {
		b, err := jsonx.Marshal(value)
		if err != nil {
			return fail(err)
		}
		fmt.Fprintln(stdout, string(b))
		return 0
	}
	switch v := value.(type) {
	case []communitySummary:
		if len(v) == 0 {
			fmt.Fprintln(stdout, "No chirps yet.")
		}
		for _, t := range v {
			fmt.Fprintf(stdout, "%s  %s  %s  (%d replies)\n", t.ID, t.Author.Username(), t.Title, t.Replies)
		}
	case community.Thread:
		fmt.Fprintf(stdout, "%s\n", v.ID)
		for _, m := range v.Messages {
			fmt.Fprintf(stdout, "%s · %s: %s\n", m.At.Format("15:04"), m.Author.Username(), m.Text)
		}
	}
	return 0
}
