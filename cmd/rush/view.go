package main

import (
	"fmt"
	"io"
	"os"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

const viewUsage = `rush view: a panel of your own, drawn by rush beside the conversation

  rush view set <name>     the view, as JSON on stdin; the same name again updates it in place
  rush view clear <name>   takes it away
  rush view list           the session's views

It's the session's in $RUSH_SESSION; --session <id> names another.

  {"title": "Pull requests",
   "summary": "8 ready · 2 failing · 1 waiting",       one line, under the session in rush's list
   "stats":  [{"value": "8", "label": "ready", "tone": "good"}],      up to 4
   "groups": [{"title": "Failing",
               "rows": [{"text": "#8947 fix the flaky ops test", "meta": "lint fails",
                         "tone": "bad", "link": "https://github.com/o/r/pull/8947"}]}],
   "note": "updated after each push"}

Tones: good, bad, warn, busy, quiet. Keep a view current as things change, rather than
repeating its contents in your messages; clear it once it's no use.`

func viewCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	id := os.Getenv("RUSH_SESSION")
	if len(args) > 1 && args[0] == "--session" {
		id, args = args[1], args[2:]
	}
	fail := func(err error) int {
		fmt.Fprintln(stderr, "rush view:", err)
		return 1
	}
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		fmt.Fprintln(stdout, viewUsage)
		return 0
	}
	if id == "" {
		return fail(fmt.Errorf("not in a rush session: --session <id> names one"))
	}
	switch {
	case args[0] == "set" && len(args) == 2:
		b, err := io.ReadAll(io.LimitReader(stdin, 1<<20))
		if err != nil {
			return fail(err)
		}
		var v host.View
		if err := jsonx.Unmarshal(b, &v); err != nil {
			return fail(fmt.Errorf("its JSON: %w (rush view help shows the shape)", err))
		}
		v.Name = args[1]
		if err := host.SaveView(id, v); err != nil {
			return fail(err)
		}
		fmt.Fprintln(stdout, "shown: "+v.Name)
		return 0
	case args[0] == "clear" && len(args) == 2:
		if err := host.DropView(id, args[1]); err != nil {
			return fail(err)
		}
		return 0
	case args[0] == "list" && len(args) == 1:
		vs, _ := host.Views(id)
		for _, v := range vs {
			fmt.Fprintf(stdout, "%s\t%s\t%s\n", v.Name, v.Title, v.Updated.Local().Format("15:04:05"))
		}
		return 0
	}
	fmt.Fprintln(stderr, viewUsage)
	return 2
}
