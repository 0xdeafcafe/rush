package main

import (
	"cmp"
	"context"
	"crypto/rand"
	"encoding/hex"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/bundled/drive"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/prdrive"
	"github.com/0xdeafcafe/rush/internal/watch"
)

// prFollowCmd is rush pr follow, unfollow and following, for the session
// rush runs this in.
func prFollowCmd(ctx context.Context, args []string, dir string, stdout, stderr io.Writer) int {
	session := os.Getenv("RUSH_SESSION")
	if session == "" {
		fmt.Fprintln(stderr, "rush pr: only a session rush runs can be followed for: wait with rush watch pr instead")
		return 1
	}
	switch args[0] {
	case "following":
		for _, e := range prdrive.Following(session) {
			fmt.Fprintf(stdout, "%s#%d since %s\n", e.Repo, e.Number, e.Started.Format("15:04 Jan 2"))
		}
		return 0
	case "unfollow":
		list := prdrive.Following(session)
		if len(args) > 1 {
			list = nil
			for _, what := range args[1:] {
				repo, n, err := watch.Resolve(ctx, dir, what)
				if err != nil {
					fmt.Fprintln(stderr, "rush pr:", err)
					return 1
				}
				list = append(list, prdrive.Entry{Session: session, Repo: repo, Number: n})
			}
		}
		for _, e := range list {
			if err := prdrive.Stop(e); err != nil {
				fmt.Fprintln(stderr, "rush pr:", err)
				return 1
			}
			fmt.Fprintf(stdout, "stopped following %s#%d\n", e.Repo, e.Number)
		}
		return 0
	}
	prs := args[1:]
	if len(prs) == 0 {
		prs = []string{""}
	}
	out, err := prdrive.FollowAll(ctx, session, prs, func(what string) (string, int, error) { return watch.Resolve(ctx, dir, what) })
	fmt.Fprintln(stdout, out)
	if err != nil {
		fmt.Fprintln(stderr, "rush pr:", err)
		return 1
	}
	return 0
}

// prFollowRun is the detached follower prdrive.Start starts.
func prFollowRun(ctx context.Context, args []string, stderr io.Writer) int {
	fs := flag.NewFlagSet("rush pr follow-run", flag.ContinueOnError)
	session := fs.String("session", "", "")
	repo := fs.String("repo", "", "")
	n := fs.Int("pr", 0, "")
	if err := fs.Parse(args); err != nil || *session == "" || *repo == "" || *n == 0 {
		fmt.Fprintln(stderr, "usage: rush pr follow-run --session ID --repo OWNER/NAME --pr N")
		return 1
	}
	e, ok := prdrive.Hold(prdrive.Entry{Session: *session, Repo: *repo, Number: *n})
	if !ok {
		return 0 // another follower has it
	}
	err := prdrive.Follow(ctx, prdrive.Options{
		Entry: e,
		Alive: func() bool { _, err := host.ReadConfig(*session); return err == nil && drive.On() }, // off ends it
		LastTurn: func() time.Time {
			i, err := host.ReadInfo(*session)
			if err != nil || i.State == "working" {
				return time.Time{}
			}
			return cmp.Or(i.IdleSince, i.UpdatedAt) // as the list ages it
		},
		Deliver: func(text string) error {
			b := make([]byte, 16)
			_, _ = rand.Read(b)
			return host.Deliver(*session, event.Exchange{ID: hex.EncodeToString(b), Direction: "received", Phase: "message", Text: text,
				Sender: event.Peer{Kind: "rush", Name: fmt.Sprintf("rush · %s#%d", *repo, *n)}}, false)
		},
	})
	if err != nil && !strings.Contains(err.Error(), "context canceled") {
		fmt.Fprintln(stderr, "rush pr follow:", err)
		return 1
	}
	return 0
}
