package main

import (
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/host"
)

const eventsUsage = `rush events: what rush did and why, newest first, a line each

  rush events [-s SESSION] [-k kind,kind] [-p proc] [--since 10m] [--grep TEXT] [-n 20] [--json]
  rush events --chain EV    one line's chain: what led to it and what came of that
  rush why SESSION [--last 1h]
                            that session's lines, its parent's, children's and
                            senders', and rush's own failures, each under its cause

  A row is: how long ago, kind, session, process:pid, #ev ←cause, then its fields.
  -k takes kind* for every kind it starts. --since takes 90s, 10m, 2h, 3d or a
  time (2026-10-23T09:00). --json prints the lines as they're kept.
`

// narrowEvents is what a cut-short answer says to narrow it with.
const narrowEvents = "-s, -k, -p, --since or --grep"

func eventsCmd(args []string, stdout, stderr io.Writer) int {
	fs := newFlags("events")
	var (
		session, kinds, proc, since, grep, chain string
		n                                        int
		raw, help                                bool
	)
	fs.StringVar(&session, "s", "", "")
	fs.StringVar(&kinds, "k", "", "")
	fs.StringVar(&proc, "p", "", "")
	fs.StringVar(&since, "since", "", "")
	fs.StringVar(&grep, "grep", "", "")
	fs.StringVar(&chain, "chain", "", "")
	fs.IntVar(&n, "n", 20, "")
	fs.BoolVar(&raw, "json", false, "")
	fs.BoolVar(&help, "h", false, "")
	if err := fs.Parse(args); err != nil || help || fs.NArg() > 0 {
		fmt.Fprint(stderr, eventsUsage)
		if help {
			return 0
		}
		return 2
	}
	now := time.Now()
	if chain != "" {
		lines, err := events.Chain(strings.TrimPrefix(chain, "#"))
		if err != nil {
			fmt.Fprintln(stderr, "rush events:", err)
			return 1
		}
		return printLines(stdout, lines, raw, now, "--json for all of it")
	}
	f := events.Filter{Proc: proc, Grep: grep, Limit: n}
	if session != "" {
		f.Sessions = strings.Split(session, ",")
	}
	if kinds != "" {
		f.Kinds = strings.Split(kinds, ",")
	}
	var err error
	if f.Since, err = events.ParseSince(since, now); err != nil {
		fmt.Fprintln(stderr, "rush events:", err)
		return 2
	}
	lines, more, err := events.Query(f)
	if err != nil {
		fmt.Fprintln(stderr, "rush events:", err)
		return 1
	}
	if len(lines) == 0 {
		fmt.Fprintln(stderr, "rush events: none match")
		return 1
	}
	if raw {
		for _, l := range lines {
			fmt.Fprintf(stdout, "%s\n", l.Raw)
		}
		return 0
	}
	fmt.Fprint(stdout, events.Format(events.Flat(lines), now, events.Budget, more, narrowEvents+", or -n to raise the limit"))
	return 0
}

// whyCmd is rush why: a session's story, each line under its cause.
func whyCmd(args []string, stdout, stderr io.Writer) int {
	fs := newFlags("why")
	var (
		last string
		raw  bool
	)
	fs.StringVar(&last, "last", "1h", "")
	fs.BoolVar(&raw, "json", false, "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		fmt.Fprintln(stderr, "usage: rush why SESSION [--last 1h] [--json]")
		return 2
	}
	now := time.Now()
	since, err := events.ParseSince(last, now)
	if err != nil {
		fmt.Fprintln(stderr, "rush why:", err)
		return 2
	}
	lines, err := events.Why(id, linkedSessions(id), since)
	if err != nil {
		fmt.Fprintln(stderr, "rush why:", err)
		return 1
	}
	if len(lines) == 0 {
		fmt.Fprintf(stderr, "rush why: nothing of %s's in the last %s\n", id, last)
		return 1
	}
	return printLines(stdout, lines, raw, now, "--last, or rush events -s "+id+" -k")
}

// printLines prints lines (oldest first) under their causes, or as kept.
func printLines(w io.Writer, lines []events.Line, raw bool, now time.Time, narrow string) int {
	if raw {
		for _, l := range lines {
			fmt.Fprintf(w, "%s\n", l.Raw)
		}
		return 0
	}
	fmt.Fprint(w, events.Format(events.Tree(lines), now, events.Budget, false, narrow))
	return 0
}

// linkedSessions are the sessions id's config and the others' say it's
// linked to: the one that started it and the ones it started.
func linkedSessions(id string) []string {
	var out []string
	if c, err := host.ReadConfig(id); err == nil && c.Meta["spawnedBy"] != "" {
		out = append(out, c.Meta["spawnedBy"])
	}
	var l host.Lister
	for _, i := range l.List() {
		if i.Meta["spawnedBy"] == id {
			out = append(out, i.ID)
		}
	}
	return out
}
