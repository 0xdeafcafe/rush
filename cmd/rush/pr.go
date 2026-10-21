package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/0xdeafcafe/rush/internal/bundled/drive"
	"github.com/0xdeafcafe/rush/internal/prdrive"
	"github.com/0xdeafcafe/rush/internal/watch"
)

const prUsage = `rush pr: drive a pull request to mergeable. rush works out what's left, in
the order to do it, and follows the PR for you, so you never poll it.

  rush pr state [<pr>...]          what's left: the verdict and the to-do list
  rush pr follow [<pr>...]         state, then rush follows each PR and sends
                                   this session a message only with news to act on
  rush pr unfollow [<pr>...]       stop following (all of this session's, with none)
  rush pr following                what this session follows
  rush pr reply <pr> <thread> <text>   one reply on a review thread, refused when
                                   it would start or carry on an argument
  rush pr resolve <pr> <thread>... resolve review threads
  rush pr help                     this, and how to drive a PR

A PR is a number (in this repository), owner/name#number or its URL; with
none, this branch's.
`

func prCmd(args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		fmt.Fprint(stdout, prUsage+"\n"+prdrive.Playbook)
		return 0
	}
	if !drive.On() {
		fmt.Fprintln(stderr, `rush pr: the drive plugin is off: "rush plugin on drive" turns it on`)
		return 1
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	dir, _ := os.Getwd()
	fail := func(err error) int {
		fmt.Fprintln(stderr, "rush pr:", err)
		return 1
	}
	resolve := func(what string) (string, int, error) { return watch.Resolve(ctx, dir, what) }
	switch args[0] {
	case "state", "status":
		prs := args[1:]
		if len(prs) == 0 {
			prs = []string{""}
		}
		var out []string
		for _, what := range prs {
			repo, n, err := resolve(what)
			if err != nil {
				return fail(err)
			}
			s, err := prdrive.Look(ctx, repo, n)
			if err != nil {
				return fail(err)
			}
			out = append(out, prdrive.Render(s))
		}
		fmt.Fprintln(stdout, strings.Join(out, "\n\n"))
		return 0
	case "reply":
		if len(args) != 4 {
			return fail(fmt.Errorf("usage: rush pr reply <pr> <thread> <text>"))
		}
		repo, n, err := resolve(args[1])
		if err != nil {
			return fail(err)
		}
		if err := prdrive.Reply(ctx, repo, n, args[2], args[3], &prdrive.Trust{Repo: repo}); err != nil {
			return fail(err)
		}
		fmt.Fprintln(stdout, "replied")
		return 0
	case "resolve":
		if len(args) < 3 {
			return fail(fmt.Errorf("usage: rush pr resolve <pr> <thread>..."))
		}
		if err := prdrive.Resolve(ctx, args[2:]...); err != nil {
			return fail(err)
		}
		fmt.Fprintf(stdout, "resolved %d\n", len(args)-2)
		return 0
	case "follow", "unfollow", "following":
		return prFollowCmd(ctx, args, dir, stdout, stderr)
	case "follow-run":
		// The detached follower rush pr follow starts: not meant to be run by hand.
		return prFollowRun(ctx, args[1:], stderr)
	}
	fmt.Fprintf(stderr, "rush pr: no %q\n\n%s", args[0], prUsage)
	return 1
}
