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
  rush view clear <name>     puts it away among the past views, where the user can bring it back
  rush view list             the session's views
  rush view guide            the style guide: when to show one, and every style
  rush view example [<name>] an example view's JSON: one for each style (table, chart, board…)
                             and for kinds of work (prs, tests, migration, search, deploy…)

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
const viewGuide = `rush view guide: the style guide

rush draws a panel beside the conversation, about 50 columns wide and as tall as the
conversation. You fill it with your view tool (or rush view set <name>, JSON on stdin). It
shows one view at a time, the others as tabs; the user scrolls it, folds a block by its title,
sorts a table by its header, opens a link by clicking its row, puts a view away with × and
brings past ones back with ⟲.

WHEN IT COMES AND GOES
  Show one on your own, without being asked, as soon as the user would otherwise follow
  something through your messages:
  - a list you were handed to work through: 30 flaky tests, the files of a rename, tickets
  - a long search: every caller of an old API, every place a setting is read
  - several pull requests, a test run, a build or CI
  - a migration or upgrade across modules or services; a deploy or rollout
  - services or processes you started; an incident or a log you're watching
  - numbers to compare: timings, sizes, costs, before and after
  - a review's findings; research options and their sources
  Not for a single answer, a short task that ends this turn, or a plan you'll say once.
  Set it again (same name) whenever something in it changes. Clear it once it's no use: it
  goes among the past views, where the user can bring it back. rush puts away a view you
  haven't touched in 3 turns by itself; one the user brought back stays until they close it.

PICK THE STYLE THAT FITS THE DATA
  A view is a title, an optional summary (your line in rush's list, 3 to 6 words), an accent
  and blocks, top to bottom. Mix styles: a hero number over a table over a callout. Each block
  has a kind, an optional title (a rule a click folds), a tone, and folded.

  TABULAR: when the user reads values
    table      Rows with the same few fields. Numbers sit right; the user sorts by a column.
               {"kind":"table","columns":["endpoint","p95","calls"],
                "items":[{"cells":["GET /orders","812ms","41k"],"tone":"bad","link":"…"}]}
    kv         Properties of one thing: an environment, a config, a release.
               {"kind":"kv","items":[{"text":"branch","meta":"feat/pay"},{"text":"db","meta":"ok","tone":"good"}]}
    checklist  Things to do in order, ticked as done; a bar of how many under it.
               {"kind":"checklist","items":[{"text":"freeze main","done":true},{"text":"sign"}]}
    log        Lines as a program wrote them, newest last; bad ones red.
               {"kind":"log","items":[{"time":"14:05:02","text":"ECONNRESET","tone":"bad"}]}
    list       Things by state with a word each: the everyday style. A row's progress, 0 to 1,
               draws a small bar. {"kind":"list","items":[{"text":"#8947 fix","meta":"lint fails","tone":"bad"}]}

  VISUAL: when the shape matters more than the values
    bars       Sizes side by side: time per job, uses per module. meta is the number's words.
               {"kind":"bars","items":[{"text":"e2e","value":9,"meta":"9m"}]}
    chart      A number over time: columns, or "style":"line" for a trend. Labels: a few, in order.
               {"kind":"chart","values":[2,3,9,22,12,4],"labels":["14:00","14:10"]}
    heatmap    A grid of how much: failures by package and day, activity by hour.
               {"kind":"heatmap","columns":["M","T","W"],"items":[{"text":"ui","values":[0,3,1]}]}
    compare    Before and after, the after bright in its tone.
               {"kind":"compare","items":[{"text":"bundle","from":"412 kB","to":"328 kB","tone":"good"}]}
    stats      Two to four headline numbers as tiles, each a one-word label and a trend.
               {"kind":"stats","stats":[{"value":"8","label":"ready","tone":"good","trend":[2,5,8]}]}
    progress   One thing moving to done. {"kind":"progress","progress":{"done":38,"total":52,"label":"moved"}}

  FLOW: when order or structure is the point
    steps      A pipeline, left to right, solid as far as it's gone.
               {"kind":"steps","items":[{"text":"build","tone":"good"},{"text":"deploy","tone":"busy"},{"text":"verify"}]}
    timeline   Events down a line at their times: an incident, a session's story.
               {"kind":"timeline","items":[{"time":"14:05","text":"502s begin","meta":"4%","tone":"bad"}]}
    board      Cards in columns by state: to do, doing, done.
               {"kind":"board","items":[{"text":"Doing","tone":"busy","items":[{"text":"TestRetry"}]}]}
    tree       Things under things: modules and files, a call tree. A click folds a branch.
               {"kind":"tree","items":[{"text":"invoices","meta":"17","items":[{"text":"create.ts","meta":"9"}]}]}

  EXPRESSIVE: when one thing should land
    hero       One number drawn big, what it is beside it, its trend under it.
               {"kind":"hero","value":"412","text":"tests passing, of 415","tone":"good","trend":[380,401,412]}
    callout    Something to notice: a warning, a decision, the one-line answer.
               {"kind":"callout","tone":"warn","text":"The US has no merchant id yet."}
    code       A few lines of code as they are. {"kind":"code","text":"max: 64, // was 32"}
    text       A sentence or two of context. {"kind":"text","text":"…"}

  The older top-level fields still work: progress, stats, groups (lists), note.

COLOUR MEANS SOMETHING
  good ✓ green done, passing · bad ✗ red broken · warn ▲ yellow needs a look ·
  busy ◐ blue running · quiet · grey not yet · none: just a fact.
  Leave things plain when a tone would say nothing. "accent" (orange, blue, green, yellow,
  purple, red) is the view's own colour, for its mark and what has no tone: pick one per kind
  of work so views look apart (red for an incident, green for a rollout, blue for performance).

ROOM
  About 46 columns inside. Short words: row text under 30, meta under 20, labels one word.
  No emoji, markdown or ASCII art: rush draws the marks, rules and bars. Fold blocks that
  matter less. Don't repeat a number in two styles.

Start from an example: rush view example lists them; rush view example table | rush view set t`

// viewExamples are views to copy: each shows a kind of work a view suits.
var viewExamples = map[string]string{
	"table":     `{"title": "Slowest endpoints", "accent": "blue", "blocks": [{"kind": "table", "columns": ["endpoint", "p95", "calls", "errors"], "items": [{"cells": ["GET /v1/orders", "812ms", "41k", "0.2%"], "tone": "bad"}, {"cells": ["POST /v1/pay", "430ms", "9k", "1.1%"], "tone": "warn"}, {"cells": ["GET /v1/me", "38ms", "120k", "0%"]}, {"cells": ["GET /v1/cart", "120ms", "66k", "0%"]}, {"cells": ["POST /v1/refund", "301ms", "800", "0.4%"]}]}]}`,
	"kv":        `{"title": "This environment", "accent": "purple", "blocks": [{"kind": "kv", "items": [{"text": "branch", "meta": "feat/apple-pay"}, {"text": "node", "meta": "22.11.0"}, {"text": "database", "meta": "postgres 17 · seeded", "tone": "good"}, {"text": "stripe", "meta": "test mode"}, {"text": "feature flag", "meta": "applePay: off in us", "tone": "warn"}, {"text": "preview", "meta": "pr-8965.acme.dev", "link": "https://x.y"}]}]}`,
	"checklist": `{"title": "Release 3.0", "accent": "green", "blocks": [{"kind": "checklist", "items": [{"text": "freeze main", "done": true}, {"text": "changelog", "done": true}, {"text": "migrations dry run", "done": true}, {"text": "staging soak", "meta": "4h of 24h", "tone": "busy"}, {"text": "sign binaries"}, {"text": "announce"}]}]}`,
	"log":       `{"title": "api.log", "accent": "red", "blocks": [{"kind": "log", "items": [{"time": "14:05:01", "text": "GET /v1/pay 200 41ms", "tone": "quiet"}, {"time": "14:05:02", "text": "ECONNRESET upstream stripe", "tone": "bad"}, {"time": "14:05:02", "text": "retrying (1/3) in 200ms", "tone": "warn"}, {"time": "14:05:03", "text": "GET /v1/pay 200 380ms"}, {"time": "14:05:04", "text": "pool exhausted: 32 of 32", "tone": "bad"}]}]}`,
	"compare":   `{"title": "Before and after", "accent": "green", "blocks": [{"kind": "compare", "items": [{"text": "bundle", "from": "412 kB", "to": "328 kB", "tone": "good"}, {"text": "first paint", "from": "1.9s", "to": "1.2s", "tone": "good"}, {"text": "build", "from": "48s", "to": "52s", "tone": "warn"}, {"text": "tests", "from": "412", "to": "431"}]}]}`,
	"bars":      `{"title": "Where the time goes", "blocks": [{"kind": "bars", "items": [{"text": "e2e", "value": 9, "meta": "9m", "tone": "warn"}, {"text": "unit", "value": 4, "meta": "4m"}, {"text": "lint", "value": 1.5, "meta": "1m 30s"}, {"text": "build", "value": 1, "meta": "1m"}]}]}`,
	"chart":     `{"title": "Errors per minute", "accent": "red", "blocks": [{"kind": "chart", "values": [2, 3, 2, 4, 3, 9, 14, 22, 19, 12, 8, 5, 4, 3, 3, 2], "labels": ["14:00", "14:08", "14:16"]}]}`,
	"line":      `{"title": "p95 latency", "accent": "blue", "blocks": [{"kind": "chart", "style": "line", "values": [180, 190, 185, 240, 260, 300, 280, 230, 215, 212, 205, 198, 190, 186], "labels": ["mon", "thu", "sun"]}]}`,
	"heatmap":   `{"title": "Test failures", "accent": "orange", "blocks": [{"kind": "heatmap", "columns": ["M", "T", "W", "T", "F", "S", "S"], "items": [{"text": "ui", "values": [0, 1, 0, 3, 2, 0, 0]}, {"text": "host", "values": [1, 0, 0, 0, 1, 0, 0]}, {"text": "plugind", "values": [4, 5, 2, 6, 3, 1, 0]}, {"text": "convo", "values": [0, 0, 1, 0, 0, 0, 0]}]}]}`,
	"steps":     `{"title": "Deploy pipeline", "blocks": [{"kind": "steps", "items": [{"text": "build", "meta": "1m", "tone": "good"}, {"text": "test", "meta": "4m", "tone": "good"}, {"text": "deploy", "meta": "eu ✓ us …", "tone": "busy"}, {"text": "verify"}]}]}`,
	"timeline":  `{"title": "Incident", "accent": "red", "blocks": [{"kind": "timeline", "items": [{"time": "14:02", "text": "deploy 2.14 to us-east", "tone": "quiet"}, {"time": "14:05", "text": "502s begin on checkout", "meta": "4% of requests", "tone": "bad"}, {"time": "14:11", "text": "paged: on-call ack", "tone": "warn"}, {"time": "14:19", "text": "db pool raised to 64", "tone": "busy"}, {"time": "14:24", "text": "errors back under 0.1%", "tone": "good"}]}]}`,
	"board":     `{"title": "Flaky tests", "accent": "yellow", "blocks": [{"kind": "board", "items": [{"text": "To do", "tone": "quiet", "items": [{"text": "TestFeedTags"}, {"text": "TestPluginApprove"}]}, {"text": "Doing", "tone": "busy", "items": [{"text": "TestRetryBackoff", "meta": "run 40×", "tone": "busy"}]}, {"text": "Done", "tone": "good", "items": [{"text": "TestExportCSV", "tone": "good"}, {"text": "TestSessionReload", "meta": "fixed a race", "tone": "good"}]}]}]}`,
	"tree":      `{"title": "Where the old API is used", "accent": "purple", "blocks": [{"kind": "tree", "items": [{"text": "invoices", "meta": "17", "items": [{"text": "create.ts", "meta": "9"}, {"text": "list.ts", "meta": "6"}, {"text": "pdf.ts", "meta": "2"}]}, {"text": "entitlements", "meta": "11", "items": [{"text": "check.ts", "meta": "11", "tone": "warn"}]}, {"text": "checkout", "meta": "7", "items": [{"text": "pay.ts", "meta": "7"}]}]}]}`,
	"hero":      `{"title": "Tests", "accent": "green", "blocks": [{"kind": "hero", "value": "412", "text": "passing, of 415", "tone": "good", "trend": [380, 395, 401, 398, 407, 412]}]}`,
	"latency":   `{"title": "Latency", "accent": "blue", "blocks": [{"kind": "hero", "value": "41ms", "text": "p50 search, down from 90ms", "trend": [90, 84, 70, 55, 44, 41]}]}`,
	"callout":   `{"title": "Heads up", "blocks": [{"kind": "callout", "tone": "warn", "text": "The US has no merchant id yet, so Apple Pay stays hidden there until finance adds one."}, {"kind": "callout", "text": "Everything else is live: 4 of 5 regions take Apple Pay."}]}`,
	"code":      `{"title": "The fix", "accent": "purple", "blocks": [{"kind": "code", "text": "const pool = new Pool({\n  max: 64,          // was 32\n  idleTimeoutMillis: 10_000,\n})"}]}`,
	"rollout":   `{"title": "Apple Pay rollout", "accent": "green", "summary": "4 of 5 regions", "blocks": [{"kind": "hero", "value": "74%", "text": "conversion with Apple Pay, card is 61%", "tone": "good"}, {"kind": "steps", "items": [{"text": "gb", "tone": "good"}, {"text": "ie", "tone": "good"}, {"text": "fr", "tone": "good"}, {"text": "de", "tone": "good"}, {"text": "us", "tone": "bad"}]}, {"kind": "callout", "tone": "bad", "text": "us: no merchant id yet."}]}`,
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
