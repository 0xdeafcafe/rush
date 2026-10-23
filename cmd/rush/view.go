package main

import (
	"fmt"
	"io"
	"maps"
	"os"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

const viewUsage = `rush view: a panel of your own, drawn by rush beside the conversation

  rush view set <name>       the view, as JSON on stdin; the same name again updates it in place
  rush view clear <name>     takes it away
  rush view list             the session's views
  rush view example [<name>] an example view's JSON: prs, tests, migration, services, deploy

It's the session's in $RUSH_SESSION; --session <id> names another. To see one drawn:

  rush view example tests | rush view set tests

  {"title": "Pull requests",
   "summary": "8 ready · 2 failing · 1 waiting",       one line, under the session in rush's list
   "tone": "good",                                      the title's mark
   "progress": {"done": 8, "total": 11, "label": "merged"},          a bar under the title
   "stats":  [{"value": "8", "label": "ready", "tone": "good",        up to 4, as tiles; plain
               "trend": [2, 3, 5, 8]}],                             numbers also share a bar
   "groups": [{"title": "Failing", "tone": "bad",
               "rows": [{"text": "#8947 fix the flaky ops test", "meta": "lint fails",
                         "tone": "bad", "link": "https://github.com/o/r/pull/8947"},
                        {"text": "#8950 rerun", "meta": "checks running",
                         "tone": "busy", "progress": 0.45}]}],      a small bar at the row's end
   "note": "updated after each push"}

Tones: good ✓, bad ✗, warn ▲, busy ◐, quiet ·. Keep a view current as things change, rather
than repeating its contents in your messages; clear it once it's no use.`

// viewExamples are views to copy: each shows a kind of work a view suits.
var viewExamples = map[string]string{
	"prs": `{"title": "Pull requests", "summary": "3 ready · 1 failing · 1 running",
 "progress": {"done": 3, "total": 5, "label": "ready to merge", "tone": "good"},
 "stats": [{"value": "3", "label": "ready", "tone": "good"}, {"value": "1", "label": "failing", "tone": "bad"},
           {"value": "1", "label": "running", "tone": "busy"}],
 "groups": [
  {"title": "Failing", "tone": "bad", "rows": [
   {"text": "#8947 fix the flaky ops test", "meta": "lint fails", "tone": "bad", "link": "https://github.com/o/r/pull/8947"}]},
  {"title": "Running", "tone": "busy", "rows": [
   {"text": "#8950 retry uploads", "meta": "6 of 13 checks", "tone": "busy", "progress": 0.46}]},
  {"title": "Ready for your approval", "tone": "good", "rows": [
   {"text": "#8965 trace filters", "meta": "all green", "tone": "good"},
   {"text": "#8942 dataset export", "meta": "all green", "tone": "good"},
   {"text": "#8939 quieter logs", "meta": "all green", "tone": "good"}]}],
 "note": "Updated after each push."}`,
	"tests": `{"title": "Test run", "summary": "412 passed · 3 failed", "tone": "warn",
 "progress": {"done": 415, "total": 460, "label": "tests run"},
 "stats": [{"value": "412", "label": "passed", "tone": "good", "trend": [380, 395, 401, 398, 407, 412]},
           {"value": "3", "label": "failed", "tone": "bad", "trend": [9, 7, 7, 5, 4, 3]},
           {"value": "45", "label": "to go", "tone": "quiet"}],
 "groups": [
  {"title": "Failed", "tone": "bad", "rows": [
   {"text": "TestRetryBackoff", "meta": "timed out after 30s", "tone": "bad"},
   {"text": "TestExportCSV/unicode", "meta": "column 4 differs", "tone": "bad"},
   {"text": "TestSessionReload", "meta": "flaky: passed on retry", "tone": "warn"}]},
  {"title": "Running", "tone": "busy", "rows": [
   {"text": "internal/ui", "tone": "busy", "progress": 0.7},
   {"text": "internal/host", "tone": "busy", "progress": 0.2}]}]}`,
	"migration": `{"title": "Move to the new event store", "summary": "migration 38 of 52 modules",
 "progress": {"done": 38, "total": 52, "label": "modules moved", "tone": "good"},
 "stats": [{"value": "38", "label": "moved", "tone": "good"}, {"value": "2", "label": "stuck", "tone": "warn"},
           {"value": "12", "label": "to do", "tone": "quiet"}],
 "groups": [
  {"title": "Stuck", "tone": "warn", "rows": [
   {"text": "billing", "meta": "replay differs on 3 events", "tone": "warn"},
   {"text": "entitlements", "meta": "waits on billing", "tone": "quiet"}]},
  {"title": "Moving now", "tone": "busy", "rows": [
   {"text": "notifications", "meta": "replaying events", "tone": "busy", "progress": 0.62}]},
  {"title": "Moved today", "tone": "good", "rows": [
   {"text": "audit log", "tone": "good"}, {"text": "search", "tone": "good"}, {"text": "users", "tone": "good"}]}],
 "note": "Each module is replayed against the old store before it moves."}`,
	"services": `{"title": "Services", "summary": "5 up · 1 down",
 "stats": [{"value": "5", "label": "up", "tone": "good"}, {"value": "1", "label": "down", "tone": "bad"},
           {"value": "212ms", "label": "p95", "trend": [180, 190, 240, 260, 230, 212]}],
 "groups": [{"rows": [
  {"text": "api", "meta": ":8080 · 2h", "tone": "good"},
  {"text": "worker", "meta": ":9000 · 2h", "tone": "good"},
  {"text": "web", "meta": ":5173 · 41m", "tone": "good"},
  {"text": "postgres", "meta": ":5432 · 3d", "tone": "good"},
  {"text": "redis", "meta": ":6379 · 3d", "tone": "good"},
  {"text": "mailer", "meta": "exited 1: no SMTP host", "tone": "bad"}]}]}`,
	"deploy": `{"title": "Deploy v2.14", "summary": "deploying: 2 of 4 regions", "tone": "busy",
 "progress": {"done": 2, "total": 4, "label": "regions", "tone": "busy"},
 "groups": [{"title": "Regions", "rows": [
  {"text": "eu-west", "meta": "healthy", "tone": "good"},
  {"text": "us-east", "meta": "healthy", "tone": "good"},
  {"text": "us-west", "meta": "rolling out", "tone": "busy", "progress": 0.35},
  {"text": "ap-south", "meta": "waits on us-west", "tone": "quiet"}]}],
 "note": "Rolls back by itself if errors pass 1%."}`,
}

func viewCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	id := os.Getenv("RUSH_SESSION")
	for i := 0; i+1 < len(args); i++ { // --session <id>, anywhere
		if args[i] == "--session" {
			id, args = args[i+1], append(args[:i:i], args[i+2:]...)
			break
		}
	}
	fail := func(err error) int {
		fmt.Fprintln(stderr, "rush view:", err)
		return 1
	}
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		fmt.Fprintln(stdout, viewUsage)
		return 0
	}
	if args[0] == "example" {
		if len(args) == 1 {
			names := slices.Sorted(maps.Keys(viewExamples))
			fmt.Fprintln(stdout, strings.Join(names, "\n"))
			return 0
		}
		ex, ok := viewExamples[args[1]]
		if !ok {
			return fail(fmt.Errorf("no example %q: rush view example lists them", args[1]))
		}
		fmt.Fprintln(stdout, ex)
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
