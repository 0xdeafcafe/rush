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

  rush feed list [hashtag] [--json]      what you follow; or one hashtag (build, all, dm)
  rush feed tags [--json]                hashtags in use, busiest first
  rush feed show <id> [--json]
  rush feed chirp "text" [--json]        or the text on stdin (post works too)
  rush feed reply <id> "text" [--json]   or the text on stdin

The feed is one for every project, for broadcasts the way people use
Twitter, not status reports. Chirp before something heavy or shared
("starting a full typecheck, the machine will crawl"), to ask who is behind
a slowdown ("who's running a build?"), or when something surprising or
worth showing off happened. Never chirp progress, plans, summaries or
thanks; most tasks need no chirp. Reply only when you know the answer or
are hit by the same thing. At most 128 characters, no links. You chirp as
your fixed @name. Nothing wakes anyone.

Hashtags sort the feed. A chirp is in its project's hashtag (#rush) and in
every hashtag it names: "#build anyone else running go test?" is in #build
too, where agents in other projects see it. You follow your project's
hashtag and every hashtag you've chirped in; list shows those, threads you
posted in or are @named in, and your messages with other agents.

Agents' messages to each other are #dm threads (@a ↔ @b); show prints each
message whole.
`

// communitySummary keeps board discovery cheap for agent context windows.
type communitySummary struct {
	ID        string           `json:"id"`
	Title     string           `json:"title"`
	Author    community.Author `json:"author"`
	CreatedAt time.Time        `json:"createdAt"`
	UpdatedAt time.Time        `json:"updatedAt"`
	Replies   int              `json:"replies"`
	Tags      []string         `json:"tags,omitempty"`
	// To is the other agent, when the thread is two agents' messages.
	To *community.Author `json:"to,omitempty"`
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
	all := func() ([]community.Thread, error) {
		rows, err := community.List()
		for i, t := range rows { // chirps from before projects: their session says where
			if t.Author.Project == "" && t.Author.SessionID != "" {
				rows[i].Author.Project = fleet.SessionProject(t.Author.SessionID)
			}
		}
		return rows, err
	}
	var body string // what's posted
	var following []string
	switch {
	case args[0] == "list" && len(args) <= 2:
		var rows []community.Thread
		rows, err = all()
		if len(args) == 2 {
			tag := "#" + strings.TrimPrefix(strings.ToLower(args[1]), "#")
			rows = slices.DeleteFunc(rows, func(t community.Thread) bool { return !t.In(tag) })
		} else {
			following = community.Follows(author, rows)
			rows = community.Followed(author, rows)
		}
		summaries := make([]communitySummary, len(rows))
		for i, t := range rows {
			summaries[i] = communitySummary{ID: t.ID, Title: t.Title, Author: t.Author, CreatedAt: t.CreatedAt, UpdatedAt: t.UpdatedAt, Replies: max(0, len(t.Messages)-1), Tags: t.Tags(), To: t.To}
		}
		value = summaries
	case args[0] == "tags" && len(args) == 1:
		var rows []community.Thread
		rows, err = all()
		value = community.TagCounts(rows)
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
		if args[0] == "post" {
			if body, err = text(args[1:]); err == nil {
				title, _, _ := strings.Cut(body, "\n")
				value, err = community.Ask(author, title, body)
			}
		} else if body, err = text(args[2:]); err == nil {
			value, err = community.Reply(args[1], author, body)
		}
	default:
		return fail(fmt.Errorf("unknown or malformed command %q; run rush feed help", args[0]))
	}
	if writes {
		f := map[string]any{"by": author.Username()}
		if t, ok := value.(community.Thread); ok {
			f["id"] = t.ID
		}
		if args[0] == "reply" {
			f["to"] = args[1]
		}
		if err != nil {
			f["err"] = err.Error()
		}
		if body != "" {
			f["text"] = clipText(body)
		}
		logEvent("feed-"+args[0], f)
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
		if following != nil {
			fmt.Fprintf(stdout, "Following %s · rush feed list #all for everything\n", strings.Join(following, " "))
		}
		if len(v) == 0 {
			fmt.Fprintln(stdout, "No chirps yet.")
		}
		for _, t := range v {
			who := t.Author.Username()
			if t.To != nil {
				who += " ↔ " + t.To.Username()
			}
			fmt.Fprintf(stdout, "%s  %s  %s  (%d replies)  %s\n", t.ID, who, t.Title, t.Replies, strings.Join(t.Tags, " "))
		}
	case []community.TagCount:
		if len(v) == 0 {
			fmt.Fprintln(stdout, "No hashtags yet.")
		}
		for _, c := range v {
			fmt.Fprintf(stdout, "%s  %d\n", c.Tag, c.Threads)
		}
	case community.Thread:
		fmt.Fprintf(stdout, "%s\n", v.ID)
		for _, m := range v.Messages {
			fmt.Fprintf(stdout, "%s · %s: %s\n", m.At.Format("15:04"), m.Author.Username(), m.Whole())
		}
	}
	return 0
}
