package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/0xdeafcafe/rush/internal/watch"
)

const watchUsage = `rush watch: wait on something without polling it yourself. rush asks for
you, cheaply, and says once, briefly, what happened.

  rush watch pr [<n> | owner/name#<n> | <url>] [--until U] [--timeout D]
                    a pull request's checks; with no PR, this branch's

  --until done      every check finished, or it merged or closed (the default)
          fail      the first check to fail, else as done
          first     the first check to finish, or a new commit
          each      a line as each check finishes, then as done: for a monitor
  --timeout 25m     give up then, saying how far it got (0: never)

It ends with: 0 all passed (or merged), 1 a check failed, 2 timed out
(run it again), 3 closed, or GitHub couldn't be asked.

It asks GitHub through gh, signed in as the user, every 15s at first and
every minute after 15m; every agent watching the same PR shares one answer,
and an answer that hasn't changed costs none of the user's rate limit.
Run it as a background command where your harness has them, so it wakes
you when it ends, and spend nothing while it waits.
`

func watchCmd(args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		fmt.Fprint(stdout, watchUsage)
		return 0
	}
	if args[0] != "pr" {
		fmt.Fprintf(stderr, "rush watch: no %q to watch\n\n%s", args[0], watchUsage)
		return watch.ExitError
	}
	fs := flag.NewFlagSet("rush watch pr", flag.ContinueOnError)
	fs.SetOutput(stderr)
	until := fs.String("until", string(watch.UntilDone), "")
	timeout := fs.Duration("timeout", 25*time.Minute, "")
	// The PR may come before or after the flags.
	var what string
	rest := args[1:]
	if len(rest) > 0 && rest[0] != "" && rest[0][0] != '-' {
		what, rest = rest[0], rest[1:]
	}
	if err := fs.Parse(rest); err != nil {
		return watch.ExitError
	}
	if fs.NArg() > 0 && what == "" {
		what = fs.Arg(0)
	}
	switch u := watch.Until(*until); u {
	case watch.UntilDone, watch.UntilFail, watch.UntilFirst, watch.UntilEach:
	default:
		fmt.Fprintf(stderr, "rush watch: --until is done, fail, first or each, not %q\n", *until)
		return watch.ExitError
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	dir, _ := os.Getwd()
	repo, n, err := watch.Resolve(ctx, dir, what)
	if err != nil {
		fmt.Fprintln(stderr, "rush watch:", err)
		return watch.ExitError
	}
	return watch.WatchPR(ctx, watch.Options{
		Repo: repo, Number: n, Until: watch.Until(*until), Timeout: *timeout,
		Say: func(s string) { fmt.Fprintln(stdout, s) },
	})
}
