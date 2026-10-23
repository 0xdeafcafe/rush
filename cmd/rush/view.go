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
  rush view guide            how to make one worth looking at: read it before your first
  rush view example [<name>] an example view's JSON: prs, tests, migration, search,
                             services, deploy, timing

It's the session's in $RUSH_SESSION; --session <id> names another. To see one drawn:

  rush view example tests | rush view set tests

  {"title": "Pull requests",
   "summary": "8 ready · 2 failing · 1 waiting",       one line, under the session in rush's list
   "tone": "good",                                      the title's mark
   "progress": {"done": 8, "total": 11, "label": "merged"},          a bar under the title
   "stats":  [{"value": "8", "label": "ready", "tone": "good",        up to 4, as tiles; plain
               "trend": [2, 3, 5, 8]}],                             numbers also share a bar
   "groups": [{"title": "Failing", "tone": "bad",
               "text": "Both fail on lint.",                    a line under its title
               "folded": false,                                 true starts it shut; a click opens it
               "rows": [{"text": "#8947 fix the flaky ops test", "meta": "lint fails",
                         "tone": "bad", "link": "https://github.com/o/r/pull/8947"},
                        {"text": "#8950 rerun", "meta": "checks running",
                         "tone": "busy", "progress": 0.45},       a small bar at the row's end
                        {"text": "e2e", "value": 9, "meta": "9m"}]}],    a chart: a bar against the
                                                                 group's largest, meta its words
   "note": "updated after each push"}

Tones: good ✓, bad ✗, warn ▲, busy ◐, quiet ·. Several views are tabs along the panel's top;
the user scrolls it, folds a group by its title and opens a row's link. Keep a view current as
things change, rather than repeating its contents in your messages; clear it once it's no use.`

// viewGuide is how to make a view worth looking at: what each part is
// for, how much fits, and what the colours mean.
const viewGuide = `rush view guide: making a view worth looking at

The panel sits beside the conversation, 42 to 64 columns wide and as tall as the conversation.
It shows one view at a time; the others are tabs along its top edge. The user scrolls it with
the wheel, folds a group by clicking its title, and opens a row's link by clicking the row.
A view is a live picture of work that changes, not a place for answers or prose.

WHEN
  Make one when there's a list the user would otherwise scroll back through your messages
  to follow, and it changes while you work. For example:

  - you were handed a list to work through: 30 flaky tests, the files of a rename, tickets,
    a checklist. Rows by where each stands: done, doing, stuck, to do.
  - a long search: grepping or reading across a large codebase for every caller, every use
    of an old API, every place a setting is read. Rows for what you found, grouped by
    module, with a stat for how many and how many are left to check.
  - several pull requests at once: each one's checks, reviews and what it waits on.
  - a test run, a build or CI: what passed, what failed and why, what's still running.
  - a migration or upgrade across modules or services: a progress bar, what's moved, what's
    stuck and on what.
  - services or processes you started: each one's port, how long it's been up, whether it's
    healthy.
  - comparing things: timings, sizes, costs, before and after. Rows with values, as a chart.
  - a review's findings: by how bad, each linked to its file or thread.

  Not for: a single answer, a short task that ends this turn, or a plan you'll only say
  once. Put those in your reply. Make it early, when you first see the list, and keep it
  current, rather than at the end.

THE PARTS, top to bottom (use the ones that say something; leave the rest out)
  progress  One thing moving towards done: "38 of 52 modules moved". One per view.
  stats     2 to 4 headline numbers. Value short ("8", "212ms", "3/5"), label one word
            ("ready", "failing", "p95"). Plain numbers also share a bar of their parts.
            trend: the number's history, oldest first, 5 to 20 points, for a sparkline.
  groups    Rows that change, grouped by what the user does next: Failing, Running, Ready,
            Done. Worst first. Title 1 to 3 words: the count is drawn for you.
            text: one short sentence on why, when the title can't say it.
            folded: true for groups the user rarely needs (Done, Next, Skipped).
  rows      text: the thing, under 30 characters ("#8947 flaky ops test", "billing").
            meta: a few words on it, under 20 ("lint fails", "since 09:12"); longer goes
            under the row. tone: its mark. link: a web page a click opens. progress: 0 to 1.
            value: a number, which turns the row into a bar against its group's largest;
            a chart, for comparing sizes (time per check, cost per module, files per area).
            Its meta is the number as you'd say it ("9m", "61%").
  note      One quiet line at the end: how the view is kept up to date.

COLOUR MEANS SOMETHING
  good   ✓ green   done, passing, live
  bad    ✗ red     broken, needs fixing
  warn   ▲ yellow  needs a look, flaky, stuck
  busy   ◐ blue    running now
  quiet  · grey    not yet, or doesn't matter
  none   • plain   just a fact
  Leave a row plain when its tone would say nothing. A view's tone is its overall state.
  Don't paint everything green: the colours only help when they differ.

ROOM
  Short words beat long ones: the panel is a third of the screen. No emoji, no markdown, no
  ASCII art: rush draws the marks, rules and bars. Don't repeat a stat as a group, or the
  title in the summary. Past ~25 rows, fold the groups that matter least.

SEVERAL VIEWS
  One per kind of work (prs, tests), up to 6. Titles of 1 to 3 words: the tabs share the top
  edge. The newest-updated one shows first.

KEEP IT CURRENT
  Set it again, same name, whenever something in it changes; it replaces the last. summary is
  your line in rush's session list: 3 to 6 words with the numbers ("3 ready · 1 failing").
  Clear it once the work is done and the user has seen the end.

Start from an example: rush view example lists them; rush view example prs | rush view set prs`

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
	"search": `{"title": "Old billing API", "summary": "41 uses · 12 modules left", "tone": "busy",
 "progress": {"done": 26, "total": 38, "label": "modules searched", "tone": "busy"},
 "stats": [{"value": "41", "label": "uses", "trend": [3, 9, 17, 28, 35, 41]}, {"value": "6", "label": "modules"},
           {"value": "2", "label": "unsure", "tone": "warn"}],
 "groups": [
  {"title": "Unsure", "tone": "warn", "text": "Called through a variable, so the name doesn't show.", "rows": [
   {"text": "reports/export.ts:88", "meta": "via handlers[kind]", "tone": "warn"},
   {"text": "jobs/nightly.ts:140", "meta": "dynamic import", "tone": "warn"}]},
  {"title": "Uses by module", "rows": [
   {"text": "invoices", "value": 17}, {"text": "entitlements", "value": 11},
   {"text": "checkout", "value": 7}, {"text": "reports", "value": 3},
   {"text": "jobs", "value": 2}, {"text": "admin", "value": 1}]},
  {"title": "Left to search", "tone": "quiet", "folded": true, "rows": [
   {"text": "notifications", "tone": "quiet"}, {"text": "search", "tone": "quiet"}]}],
 "note": "Each use is checked by reading the call, not just grep."}`,
	"timing": `{"title": "CI time", "summary": "e2e is 9 of 16 minutes", "tone": "warn",
 "stats": [{"value": "16m", "label": "total", "trend": [11, 12, 12, 14, 15, 16]}, {"value": "9m", "label": "e2e", "tone": "warn"}],
 "groups": [
  {"title": "Per job", "text": "The longest job on main's last 10 runs.", "rows": [
   {"text": "e2e", "value": 9, "meta": "9m", "tone": "warn"},
   {"text": "unit", "value": 4, "meta": "4m"},
   {"text": "lint", "value": 1.5, "meta": "1m 30s"},
   {"text": "build", "value": 1, "meta": "1m"}]},
  {"title": "Ideas", "folded": true, "rows": [
   {"text": "shard e2e across 3 runners", "meta": "−6m", "tone": "good"},
   {"text": "cache the browser download", "meta": "−40s"}]}],
 "note": "From the last 10 runs on main."}`,
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
	if args[0] == "guide" {
		fmt.Fprintln(stdout, viewGuide)
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
		if strings.Contains(args[1], "--") {
			return fail(fmt.Errorf("a view's name has no -- in it: those are plugins'"))
		}
		v.Name, v.By = args[1], "" // only rush says a plugin drew it
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
