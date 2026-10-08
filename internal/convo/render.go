package convo

import (
	"encoding/json/jsontext"
	"fmt"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/cellw"
	"github.com/0xdeafcafe/rush/internal/jsonx"

	"github.com/0xdeafcafe/rush/internal/agtools"
)

// Line is one drawn row. Ref names what it belongs to, for selection and the
// mouse: "t13" for a turn heading, "t13:s:<tool id>" for a step,
// "t13:run:4" for a folded run of steps. Empty for rows you can't act on.
type Line struct {
	Text string
	Ref  string
	// Wrap is set on a row that carries on the line above it, where the
	// text was wrapped to fit rather than broken, so copying it joins them.
	Wrap bool
}

// Depth is how much of a turn's work shows, shallowest first: Prose is
// what's said and a row for each run of steps between; Runs folds every
// run of steps to a row; Default folds runs of clean ones. Verbose, deeper
// still, opens everything.
type Depth int8

const (
	DepthDefault Depth = iota
	DepthProse
	DepthRuns
)

// Options say how to draw.
type HistoryMode int

const (
	HistoryAuto HistoryMode = iota
	HistoryOpen
	HistoryCompact
)

type Options struct {
	History      HistoryMode
	Width        int
	HideActivity bool // the pane places activity separately, outside scroll storage
	Now          time.Time
	Tick         int
	Open         map[string]bool // fold overrides by ref; absent means the default
	Verbose      bool            // ctrl+o: open everything, trim nothing
	Depth        Depth
	Selected     string
	Focused      bool
	Marks        map[string]bool // files you've marked reviewed in the changes view
	// View is the view a step's output was switched to, by ref: ViewText,
	// ViewPretty, ViewHex or ViewImage; absent means the one it opens in.
	View map[string]string
	// Wide lets rows run the whole width, past capRow: for a session shown
	// alone on a wide screen, where the right edge is the screen's.
	Wide bool
	// Budget is how long a render may take drawing afresh; 0 is no limit.
	// Past it, a turn or part of a running turn already drawn another way
	// (at the width before a resize, say) is used as it was, and Stale
	// says so: the caller draws again soon, and each render redraws more.
	Budget time.Duration
	// NoRule leaves the turns' rule out of the gutter: the pane has a
	// border just left of it, and two lines side by side read as a fold.
	NoRule bool
	// Compaction is where the session's agent compacts its context, when
	// it's short of the model's window.
	Compaction agent.Compaction
	// Parent names the session this one works for as a subagent, for the
	// card it opens on.
	Parent string
	// Hosted is whether rush runs the session, and so added its own
	// prompt to the agent's; Brief is what the session's start added
	// after it.
	Hosted bool
	Brief  string
	// Agent names who answers under each of your messages.
	Agent string
	// Paused are the Bash calls, by tool call ID, whose running command
	// rush has paused.
	Paused map[string]bool
	// Scripts are the Bash calls, by tool call ID, rush ran as scripts.
	Scripts map[string]*ScriptView
}

// rowCap is how wide a row's numbers and rules may run.
func (o Options) rowCap() int {
	if o.Wide {
		return max(o.Width, capRow)
	}
	return capRow
}

const (
	capRow   = 124 // numbers never drift further right than this
	capProse = 120 // prose wraps here however wide the pane
	gutter   = 4   // room for tool status and nested group rails
)

type cached struct {
	key   cacheKey
	lines []Line
	fast  bool // it drew a timer showing tenths: it's keyed to the tenth
	waits bool // it read a lookup not yet in: it's keyed to lookupsGen
}

// cacheKey is everything that affects a turn's drawing: its content, the
// width, its folds, the selection inside it, and for a live turn the clock.
type cacheKey struct {
	width, ver   int
	wide         bool
	pal          int // the palette it was drawn in
	open, verb   bool
	depth        Depth
	hideActivity bool
	noRule       bool
	agent        string
	paused       int
	scripts      string
	folds, sel   string
	tick         int
	now          int64
	tenth        int64 // the clock to a tenth, while a timer shows them
	gen          int64 // lookups finished: a commit card or thumbnail may read differently
	clock        bool
	latest       string // the session's newest step, when it's in this turn
	opening      string // what the card atop the first turn reads
}

// Render draws every turn, oldest first.
func (s *Session) Render(o Options) []Line { return s.RenderInto(o, nil) }

// RenderInto is Render writing into buf's storage, so a caller that draws
// every frame reuses one slice instead of allocating the whole session's
// lines each time. The result is only good until the next call.
func (s *Session) RenderInto(o Options, buf []Line) []Line {
	s.Fast = false
	if o.Width < 20 {
		o.Width = 20
	}
	s.memoTurn()
	folds := foldsByTurn(o.Open, o.View)
	latest, latestIn := s.latestAt()
	if cap(s.parts) < len(s.Turns) {
		s.parts = make([][]Line, len(s.Turns))
	}
	parts := s.parts[:len(s.Turns)]
	s.stale, s.drew, s.deadline = false, false, time.Time{}
	if o.Budget > 0 {
		s.deadline = time.Now().Add(o.Budget)
	}
	// Newest first: the end is what's on screen, so it's drawn first
	// when there's only time for some.
	n := 0
	for i := len(s.Turns) - 1; i >= 0; i-- {
		recent := o.History == HistoryOpen || i == len(s.Turns)-1
		var mine *Step
		if s.Turns[i] == latestIn {
			mine = latest
		}
		parts[i] = s.turn(s.Turns[i], o, recent, folds, mine)
		n += len(parts[i])
	}
	out := buf[:0]
	if cap(out) < n+1 {
		out = make([]Line, 0, n+n/4+1)
	}
	if s.Partial {
		out = append(out, Line{Text: dim("  reading earlier…")})
	}
	for _, p := range parts {
		out = append(out, p...)
	}
	clear(parts) // don't keep turns' lines alive through this slice
	return out
}

// foldsByTurn groups the fold and view overrides by the turn they're in,
// each turn's sorted, so a turn's cache key names only its own.
func foldsByTurn(open map[string]bool, views map[string]string) map[string]string {
	if len(open) == 0 && len(views) == 0 {
		return nil
	}
	by := map[string][]string{}
	for k, v := range open {
		t, _, _ := strings.Cut(k, ":")
		by[t] = append(by[t], k+"="+strconv.FormatBool(v))
	}
	for k, v := range views {
		t, _, _ := strings.Cut(k, ":")
		by[t] = append(by[t], k+"~"+v)
	}
	out := make(map[string]string, len(by))
	for t, fs := range by {
		sort.Strings(fs)
		out[t] = strings.Join(fs, ",")
	}
	return out
}

// latest is the newest step the session has drawn at the top level, which
// shows opened until a newer one takes its place.
func (s *Session) latest() *Step {
	st, _ := s.latestAt()
	return st
}

// latestAt is latest and the turn it's in.
func (s *Session) latestAt() (*Step, *Turn) {
	for i := len(s.Turns) - 1; i >= 0; i-- {
		items := s.Turns[i].Items
		for j := len(items) - 1; j >= 0; j-- {
			if it := items[j]; it.Kind == KStep && !hidden(it.Step) {
				return it.Step, s.Turns[i]
			}
		}
	}
	return nil, nil
}

// StepOpen is whether a step's row draws opened when nothing's overridden
// it: in verbose, or when it's the session's newest step.
func (s *Session) StepOpen(ref string, verbose bool) bool {
	if verbose {
		return true
	}
	st := s.latest()
	if st == nil {
		return false
	}
	d := drawer{s: s}
	if name, _, _ := d.skillInfo(st); name != "" {
		return false
	}
	for _, t := range s.Turns {
		for _, it := range t.Items {
			if it.Step == st {
				// A message shows its start until you open it, newest or not.
				return ref == "t"+strconv.Itoa(t.N)+":s:"+st.ID && !messageTool(st.Tool)
			}
		}
	}
	return false
}

// turn draws t, or gives it as last drawn when nothing it reads changed.
// mine is the session's newest step when it's in t.
func (s *Session) turn(t *Turn, o Options, recent bool, folds map[string]string, mine *Step) []Line {
	if t.ref == "" {
		t.ref = "t" + strconv.Itoa(t.N)
	}
	ref := t.ref
	open := recent || t.Live || t.Err != "" || waiting(t)
	if v, ok := o.Open[ref]; ok {
		open = v
	}
	key := s.cacheKey(t, o, ref, open, folds)
	if mine != nil {
		key.latest = mine.ID
	}
	c, ok := s.cache[t]
	if ok && c.fast && key.clock {
		key.tenth = o.Now.UnixMilli() / 100
	}
	if ok && !c.waits {
		key.gen = c.key.gen // a lookup landing can't change what read none
	}
	if ok && c.key == key {
		s.Fast = s.Fast || c.fast // its tenths still tick
		return c.lines
	}
	// Built only on a miss: it escapes, so every turn every frame allocated.
	d := &drawer{s: s, t: t, o: o, ref: ref, cw: min(o.Width, o.rowCap()), latest: mine}
	if ok && s.over() {
		s.Fast = s.Fast || c.fast
		s.stale = true
		return c.lines // as it was drawn: another render redraws it
	}
	if ok {
		d.lines = make([]Line, 0, len(c.lines)+8) // about as long as it was
	}
	// An open turn is drawn part by part through the unit memo: a turn on
	// the clock redraws every frame, and only what runs in it is drawn
	// anew; one laid out for a new width can be, a slice a render.
	if open && !noUnitMemo {
		d.unit = &unitKey{base: s.Info.Cwd + "|" + s.Cwd, folds: key.folds, sel: key.sel, width: o.Width, cw: d.cw, verb: o.Verbose, depth: o.Depth, pal: palette, gen: lookupsGen.Load(), spine: d.spine()}
	}
	key.gen = lookupsGen.Load()
	waits := lookupWaits.Load()
	fastBefore := s.Fast
	s.Fast = false // set again if this turn draws a timer in tenths
	_, chosen := o.Open[ref]
	switch {
	case open:
		d.open()
	case chosen:
		d.folded() // closed by hand: the message and a row
		s.drew = true
	default:
		d.settled() // gone past: what matters of it
		s.drew = true
	}
	d.lines = squeezed(d.lines)
	// An open turn draws its own gap above; previews provide their own.
	fast := s.Fast
	s.Fast = s.Fast || fastBefore
	if d.stale {
		s.stale = true // part of it is as it was: not to keep
		return d.lines
	}
	if fast && key.clock {
		key.tenth = o.Now.UnixMilli() / 100
	}
	s.cache[t] = cached{key: key, lines: d.lines, fast: fast, waits: d.waits || lookupWaits.Load() != waits}
	return d.lines
}

// over is whether the render has used its budget. It hasn't until it's
// drawn something afresh, so each render gets further than the last.
func (s *Session) over() bool {
	return s.drew && !s.deadline.IsZero() && time.Now().After(s.deadline)
}

// Stale is whether the last render ran out of budget and used some of
// what it drew before as it was, perhaps at another width: another render
// draws more of it afresh.
func (s *Session) Stale() bool { return s.stale }

func (s *Session) cacheKey(t *Turn, o Options, ref string, open bool, folds map[string]string) cacheKey {
	k := cacheKey{width: o.Width, wide: o.Wide, ver: t.ver, open: open, verb: o.Verbose, depth: o.Depth, hideActivity: o.HideActivity, noRule: o.NoRule, agent: o.Agent, scripts: scriptsSig(o.Scripts), folds: folds[ref] + folds["exchange"], pal: palette, gen: lookupsGen.Load()}
	exchangeSelected := false
	if strings.HasPrefix(o.Selected, "exchange:") {
		parts := strings.Split(o.Selected, ":")
		if len(parts) >= 3 {
			exchangeSelected = s.exchanges[parts[1]+":"+parts[2]].turn == t
		}
	}
	if exchangeSelected || o.Selected == ref || strings.HasPrefix(o.Selected, ref) && strings.HasPrefix(o.Selected[len(ref):], ":") {
		k.sel = o.Selected // focus draws no differently
	}
	if t.Live || waiting(t) {
		k.clock, k.tick, k.now, k.paused = true, o.Tick, o.Now.Unix(), len(o.Paused)
	}
	if len(s.Turns) > 0 && s.Turns[0] == t {
		k.opening = s.openingPrint(o)
	}
	return k
}

// waiting is whether a step in the turn waits on you. Every change to a
// step touches its turn, so the answer holds until the turn changes.
func waiting(t *Turn) bool {
	if t.waitVer == t.ver+1 {
		return t.waits
	}
	t.waits = false
	for _, st := range t.steps {
		if st.Status == Waiting {
			t.waits = true
			break
		}
	}
	t.waitVer = t.ver + 1
	return t.waits
}

type drawer struct {
	named  bool // a box of queued messages has said "you" once
	docked bool // live activity shown independently of the transcript
	// rule is whether the turn being drawn has its rule in the gutter: an
	// open turn does; an answer drawn for a panel doesn't.
	rule  bool
	s     *Session
	t     *Turn
	o     Options
	ref   string
	cw    int
	lines []Line
	// worked is whether a step has been drawn yet, so the answer after
	// them stands further apart.
	worked bool
	// marks are what the command being drawn echoes between parts of its
	// output, drawn as headings.
	marks map[string]bool
	// lg is the language of the output being drawn, when it's code read
	// from a file; byPath has each line say its own file (grep's a.go:12:).
	lg     *lang
	byPath bool
	hs     hlState
	// hsPath and hsN are the file and line the highlighter's state is from.
	hsPath string
	hsN    int
	// spans are the languages of a chain's output, part by part.
	spans []span
	// subject is whether the next line of a git log's message is its title.
	subject bool
	// view is what the output being drawn was switched to: ViewText or
	// ViewPretty, or "" to draw it as it opens.
	view string
	// nowrap is whether the output being drawn keeps each line to one row.
	nowrap bool
	// latest is the session's newest step when it's in this turn: it
	// shows opened, and never folds into a run.
	latest *Step
	// unit is how the turn is being drawn, for the unit memo; nil when
	// it's drawn folded. above is the
	// last row as a unit's key has it, while it's known: see rowAbove.
	unit  *unitKey
	waits bool // a unit copied from the memo read a lookup not yet in
	above int8
	// stale is whether a unit was copied as drawn another way, past the
	// render's budget: the turn isn't kept then.
	stale bool
}

// freshTail is how many of a running turn's last items are drawn afresh
// however long a render has taken: they're what's on screen.
const freshTail = 24

// spine is column 0, the gutter: the turn's one quiet rule, or the cursor's
// brighter mark on a selected row. It never changes colour with the turn.
func (d *drawer) spine() string {
	if d.rule && !d.o.NoRule {
		return spineBar
	}
	return " "
}

// add appends a row; the selected one gets the cursor in the gutter.
func (d *drawer) add(ref, b, left, right string) {
	if ref != "" && ref == d.o.Selected {
		left = cursor() + strings.TrimPrefix(left, d.spine())
	}
	d.lines = append(d.lines, Line{Text: row(b, left, right, d.o.Width, d.cw), Ref: ref})
}

// squeezed drops a blank row that follows another: markdown and tool output
// bring runs of them, and a row is only ever left clear once. A row that is
// something's own (a message's fill) stays.
func squeezed(ls []Line) []Line {
	out := ls[:0]
	prev := false
	for _, l := range ls {
		blank := l.Ref == "" && strings.Trim(stripANSI(l.Text), " ▏│") == ""
		if blank && prev {
			continue
		}
		prev = blank
		out = append(out, l)
	}
	return out
}

// answers lays out each question asked, quiet, and its answer under it.
func (d *drawer) answers(qa [][2]string, indent int) {
	pad := d.spine() + strings.Repeat(" ", indent-1)
	w := d.cw - indent - 2
	for i, p := range qa {
		if i > 0 {
			d.blank()
		}
		d.addRows("", pad, faint("? "), faint(p[0]), w, 0)
		d.addRows("", pad, paint(cGreen, "→ "), text(p[1]), w, 0)
	}
}

// wrapped marks the row just added as carrying on the one before it.
func (d *drawer) wrapped() { d.lines[len(d.lines)-1].Wrap = true }

func (d *drawer) blank() {
	d.lines = append(d.lines, Line{Text: row("", d.spine(), "", d.o.Width, d.cw)})
}

// mark is how a turn ended, when that's worth a glyph and a space: a stop
// or a failure. A turn that went fine says nothing.
func (d *drawer) mark() string {
	switch t := d.t; {
	case t.Stopped:
		return dim("⏹ ")
	case t.Err != "":
		return paint(cRed, "✗ ")
	}
	return ""
}

func (d *drawer) meta() string {
	t := d.t
	end := t.End
	if t.Live {
		end = d.o.Now
	}
	var parts []string
	if n := t.Steps() + t.agentSteps(); n > 0 {
		parts = append(parts, plural(n, "step"))
	}
	if !t.Start.IsZero() && end.Sub(t.Start) >= 100*time.Millisecond {
		parts = append(parts, dur(end.Sub(t.Start)))
	}
	if m := money(t.Cost); m != "" {
		parts = append(parts, m)
	}
	return strings.Join(parts, "   ")
}

// imageNames names a turn's images: their files, or Image #1, #2 for the
// ones a transcript only knows were there.
func imageNames(images []string) []string {
	out := make([]string, len(images))
	for i, im := range images {
		out[i] = im
		if im == "image" || im == "" {
			out[i] = fmt.Sprintf("Image #%d", i+1)
		} else {
			out[i] = ImageLabel(im)
		}
	}
	return out
}

var screenshotRe = regexp.MustCompile(`^(?:Screenshot|Screen Shot|CleanShot) \d{4}-\d{2}-\d{2} at (.+)\.(?i:png|jpe?g)$`)

// ImageLabel names an image file for a chip: its file name, with a macOS
// screenshot's long "Screenshot 2026-09-24 at 10.21.03.png" cut to the
// time that tells it from its neighbours.
func ImageLabel(path string) string {
	name := filepath.Base(path)
	if m := screenshotRe.FindStringSubmatch(strings.ReplaceAll(name, "\u202f", " ")); m != nil {
		return "screenshot " + m[1]
	}
	// An image pasted as data is saved by rush as clipboard-<time>.png.
	if m := clipboardRe.FindStringSubmatch(name); m != nil {
		return "screenshot " + m[1] + ":" + m[2]
	}
	return name
}

var clipboardRe = regexp.MustCompile(`^clipboard-\d{8}-(\d{2})(\d{2})\d{2}(?:\.\d+)?\.png$`)

// imageChips lays images out as chips, as many to a row as fit in w, a
// chip never broken across rows.
func imageChips(names []string, w int, refs ...string) []string {
	var rows []string
	row, n := "", 0
	for i, name := range names {
		cw := 2 + len([]rune(name))
		if n > 0 && n+3+cw > w {
			rows, row, n = append(rows, row), "", 0
		}
		if n > 0 {
			row, n = row+"   ", n+3
		}
		chip := paint(cBlue, "▣ ") + paint(cText, name)
		if len(refs) > 0 {
			chip = "\x1b]8;;rush:message/" + refs[0] + "/image/" + strconv.Itoa(i+1) + "\x1b\\" + chip + "\x1b]8;;\x1b\\"
		}
		row, n = row+chip, n+cw
	}
	if n > 0 {
		rows = append(rows, row)
	}
	return rows
}

// unasked names a turn with no message rush saw start it. The first is
// the replay beginning partway through a turn, its start left in the
// transcript; any later one is the agent waking for something it didn't
// say, such as background work finishing.
func unasked(t *Turn) string {
	if t.Cause != "" {
		return t.Cause
	}
	if t.N == 1 {
		return "continued from earlier"
	}
	return "woke up without a message"
}

// woke is a turn a task woke, in Claude Code's words: "Background
// command", and how it ended, " completed" in green or " failed" in red.
// switched is whether t is rush carrying a session on after moving it to
// an account with room (host.LimitContinue), drawn as rush's, not yours.
func switched(t *Turn) bool {
	return strings.HasPrefix(t.Prompt, "continue: you're on another account now, with room")
}

// switchedAsk is what such a turn says instead of rush's words to the agent.
const switchedAsk = "moved to an account with room · carrying on"

func woke(t *Turn) (noun, how string, ok bool) {
	if t.replied != "" && t.Cause != "" {
		return "⇉ " + t.replied, " " + dim("replied"), true
	}
	kind, status, _ := strings.Cut(t.From, " · ")
	noun, ok = map[string]string{
		"background shell": "Background command", "background subagent": "Background agent",
		"background workflow": "Background workflow", "monitor": "Monitor",
	}[kind]
	if !ok || t.Cause == "" {
		return "", "", false
	}
	switch status {
	case "":
	case "completed":
		how = " " + paint(cGreen, status)
	case "failed":
		how = " " + paint(cRed, status)
	default:
		how = " " + dim(status)
	}
	return noun, how, true
}

// head is the start of a turn, open or folded: a gap, then what you said in
// its box, in full, or the line for what woke it. It's drawn the same
// either way, so folding a turn never moves it.
func (d *drawer) head() {
	t := d.t
	d.opening()
	d.hooks()
	d.rule = true
	// A gap above, outside the turn's rule.
	d.lines = append(d.lines, Line{Text: row("", "", "", d.o.Width, d.cw)})
	// Images show as the box showed them, unless the words already place
	// them ([Image #1]).
	var imgs []string
	if len(t.Images) > 0 && !strings.Contains(t.Prompt, "[Image #") {
		imgs = imageNames(t.Images)
	}
	if t.foldedOf != t.Prompt { // a live turn is drawn every frame
		t.folded, t.foldedOf = FoldPastes(t.Prompt), t.Prompt
	}
	ask := t.folded
	if ask == "" {
		ask = unasked(t)
	}
	// A command of more than a line is drawn under the header as a shell
	// step's is: kept as written, highlighted, a long heredoc folded.
	multi := strings.Contains(t.Command, "\n")
	if multi {
		ask = ""
	}
	noun, how, wake := woke(t)
	yours := t.From == "" && !wake && !switched(t) && (strings.TrimSpace(t.Prompt) != "" || len(imgs) > 0 || multi)
	if yours {
		ask = strings.TrimSpace(t.Prompt) // pastes show as themselves
		if multi {
			ask = ""
		}
		d.userBox(d.ref, "", ask, imgs, SentImages(t.Pictures, t.Images), multi)
	} else {
		d.otherAsk(ask, noun, how, wake)
	}
	if multi {
		d.shellBody(&Step{}, t.Command, gutter+1)
	}
}

func (d *drawer) open() {
	d.head()
	d.speaker()
	t := d.t
	if a := d.wokeAgents(); a != nil {
		d.replies(a, d.ref+":reply", 4)
		d.blank()
	}

	items := t.Items
	// A running turn keeps its last few steps in view; everything clean
	// before them folds, so a busy agent doesn't flood the screen.
	keep := len(items)
	if t.Live {
		seen := 0
		for i := len(items) - 1; i >= 0; i-- {
			if items[i].Kind == KStep {
				seen++
				if seen == 3 {
					keep = i
					break
				}
			}
		}
		if seen < 3 {
			keep = 0
		}
	}
	if d.storied() {
		d.story() // done: the story of it, as a turn gone past has it
	} else {
		d.drawItems(items, 0, len(items), keep)
	}
	if t.Live && !d.o.HideActivity {
		d.liveLine()
	}
	switch {
	case t.Stopped:
		d.add("", "", d.spine()+"   "+dim("⏹ stopped"), "")
	case t.Err != "" && !t.Live:
		d.errCard(t.Err)
	}
	// The rail stops at the last thing drawn, never on an empty row.
	for n := len(d.lines); n > 1 && strings.TrimSpace(stripANSI(d.lines[n-1].Text)) == strings.TrimSpace(stripANSI(d.spine())) && d.lines[n-1].Ref == ""; n-- {
		d.lines = d.lines[:n-1]
	}
	// What the turn cost, once it's done: one dim line at its foot.
	if m := d.meta(); m != "" && !t.Live && t.Steps()+t.agentSteps() > 0 {
		d.add("", "", d.spine(), dim(m)+"  ")
	}
}

// drawItems draws items[lo:hi] of an open turn: steps folded into runs as
// the depth has it. keep is where the steps a running turn keeps in view
// begin.
func (d *drawer) drawItems(items []*Item, lo, hi, keep int) {
	prose, runs := !d.o.Verbose && d.o.Depth == DepthProse, !d.o.Verbose && d.o.Depth == DepthRuns
	for i := lo; i < hi; i++ {
		it := items[i]
		// Prose alone: steps go, but for a row saying how many failed.
		if prose && it.Kind == KStep {
			j, failed := i, 0
			for ; j < hi && items[j].Kind == KStep; j++ {
				if items[j].Step.Status == Failed {
					failed++
				}
			}
			if failed > 0 {
				d.add(d.ref+":run:"+strconv.Itoa(i), "", d.spine()+blanks(gutter-1)+paint(cRed, "✗ "+strconv.Itoa(failed)+" of "+plural(j-i, "step")+" failed"), "")
			}
			i = j - 1
			continue
		}
		// A run of two or more clean steps folds to one row (any run, when
		// folding runs); a failure never folds.
		if !d.o.Verbose && it.Kind == KStep && i < keep {
			j := i
			for j < keep && j < hi && items[j].Kind == KStep && (runs || foldable(items[j].Step) && items[j].Step != d.latest) && !d.testsFailed(items[j].Step) && items[j].Step.Status != Failed && !d.replyOf(items[j].Step) {
				j++
			}
			if j-i >= 2 || runs && j > i {
				run, runRef := items[i:j], d.ref+":run:"+strconv.Itoa(i)
				d.memoized(run, runRef, i >= len(items)-freshTail, func() {
					if !d.o.Open[runRef] {
						d.run(runRef, run)
						// What the run did that you'd want to see stays out.
						for _, x := range run {
							d.cards(x.Step, gutter+2)
						}
						return
					}
					// Opened: every step of the run, under a row that folds
					// it back, and nothing after it refolds.
					d.railed(gutter-2, true, func() {
						d.add(runRef, "", d.spine()+blanks(gutter-1)+faint("▾ hide "+plural(len(run), "step")), "")
						for _, x := range run {
							d.step(x.Step, 0)
						}
					})
				})
				i = j - 1
				continue
			}
		}
		// A reply read turns into a card once its agents are found: never kept.
		if it.Kind == KStep && d.replyOf(it.Step) {
			d.item(it)
			continue
		}
		d.memoized(items[i:i+1], "", i >= len(items)-freshTail, func() { d.item(it) })
	}
}

// item draws one thing in an open turn.
func (d *drawer) item(it *Item) {
	switch it.Kind {
	case KText:
		if it.Answer {
			d.answer(it.Text)
		} else {
			// Narration is the thread you read: set apart from the
			// steps around it, which stay close together.
			d.gap()
			d.prose(it.Text, 2, cSub)
			d.gap()
		}
	case KThinking:
		// Thinking shows while it happens; afterwards only in verbose.
		// While it happens, the live line at the end says so.
		if d.o.Verbose {
			d.add("", "", d.spine()+strings.Repeat(" ", gutter-1)+dim("✻ thought"), "")
			if strings.TrimSpace(it.Text) != "" {
				d.prose(it.Text, gutter+2, cDim)
			}
		}
	case KCompact:
		d.compacted(it)
	case KNotice:
		d.notice(it)
	case KExchange:
		d.exchange(it)
	case KInterject:
		d.interject(it)
	case KStep:
		d.step(it.Step, 0)
	}
}

// notice is Claude Code telling you something, as loudly as it says.
func (d *drawer) notice(it *Item) {
	col, mark := cDim, "·"
	switch it.Level {
	case "warning":
		col, mark = cYellow, "●"
	case "error":
		col, mark = cRed, "●"
	case "ok":
		col, mark = cGreen, "✓"
	}
	for k, r := range wrap(oneLine(it.Text), min(d.cw-10, capProse)) {
		lead := paint(col, mark) + " "
		if k > 0 {
			lead = "  "
		}
		d.add("", "", d.spine()+"   "+lead+paint(col, r), "")
		if k > 0 {
			d.wrapped()
		}
	}
}

// interject is what you said mid-turn: a box like the turn's own, with room
// either side.
func (d *drawer) interject(it *Item) {
	d.gap()
	d.userBox(d.t.messageRef(it), "", strings.TrimSpace(it.Text), imageNames(it.Images), SentImages(it.Pictures, it.Images), false)
}

// unitKey names a finished part of a running turn as drawn: an item, or a
// run of steps folded together, how far along it had got, and what of the
// drawing before it that it reads: whether the row above is a gap, whether
// work has been drawn yet, and the highlighter's state.
type unitKey struct {
	it     *Item
	n      int    // items in it
	ref    string // a folded run's
	print  uint64 // what it had, from unitPrint
	latest bool   // it holds the session's newest step
	above  int8   // the row above: 0 none, 1 a gap, 2 anything else
	// The drawer's state going in.
	worked, subject bool
	hs              hlState
	hsPath          string
	hsN             int
	// How the turn is drawn: set once for it.
	folds, sel, spine, base string
	verb                    bool
	depth                   Depth
	width, cw, pal          int
	gen                     int64
}

// unitDrawn is a unit as last drawn, kept on its first item: its rows,
// the row above the next one, and the drawer's state coming out.
type unitDrawn struct {
	key             unitKey
	lines           []Line
	above           int8
	worked, subject bool
	hs              hlState
	hsPath          string
	hsN             int
	waits           bool // it read a lookup not yet in: it's keyed to lookupsGen
}

// memoized draws items with draw, or copies how they were last drawn when
// nothing they read has changed since. It's only for what's done: a
// running or waiting step reads the clock, and is drawn every time.
//
// Past the render's budget, a unit drawn before another way (at another
// width, say) is copied as it was, unless it's in the turn's last few
// items, which are on screen.
func (d *drawer) memoized(items []*Item, ref string, tail bool, draw func()) {
	if d.unit == nil {
		draw()
		return
	}
	print, ok := unitPrint(items)
	if !ok {
		draw()
		d.above = 0
		return
	}
	k := *d.unit
	k.it, k.n, k.ref, k.print = items[0], len(items), ref, print
	k.latest = d.latest != nil && items[0].Step == d.latest
	k.above = d.rowAbove()
	k.worked, k.subject, k.hs, k.hsPath, k.hsN = d.worked, d.subject, d.hs, d.hsPath, d.hsN
	u := items[0].drawn
	if u != nil && !u.waits {
		k.gen = u.key.gen // a lookup landing can't change what read none
	}
	if u != nil && u.key != k && !tail && d.s.over() {
		d.stale = true
		d.lines = append(d.lines, u.lines...)
		d.worked, d.subject, d.hs, d.hsPath, d.hsN = u.worked, u.subject, u.hs, u.hsPath, u.hsN
		d.above = u.above
		return
	}
	if u == nil || u.key != k {
		from, waits := len(d.lines), lookupWaits.Load()
		k.gen = lookupsGen.Load()
		draw()
		u = &unitDrawn{key: k, lines: append([]Line(nil), d.lines[from:]...), worked: d.worked, subject: d.subject, hs: d.hs, hsPath: d.hsPath, hsN: d.hsN, waits: lookupWaits.Load() != waits}
		d.above = 0
		u.above = d.rowAbove()
		items[0].drawn = u
		d.s.drew = true
	} else {
		d.lines = append(d.lines, u.lines...)
		d.worked, d.subject, d.hs, d.hsPath, d.hsN = u.worked, u.subject, u.hs, u.hsPath, u.hsN
		d.waits = d.waits || u.waits
	}
	d.above = u.above
}

// rowAbove is what the last row drawn is, as a unit's key has it: known
// from the unit before when it came from the memo, as reading a row is
// dearer than the lookup.
func (d *drawer) rowAbove() int8 {
	switch n := len(d.lines); {
	case d.above != 0:
		return d.above
	case n == 0:
		return 0
	case d.isBlank(n - 1):
		d.above = 1
	default:
		d.above = 2
	}
	return d.above
}

// unitPrint is a fingerprint of what items have, and whether they're done
// changing with the clock: none of their steps (nor theirs) runs or waits.
func unitPrint(items []*Item) (uint64, bool) {
	h := uint64(14695981039346656037)
	mix := func(v uint64) { h = (h ^ v) * 1099511628211 }
	var step func(st *Step) bool
	step = func(st *Step) bool {
		if st.Status == Running || st.Status == Waiting || st.runLive {
			return false
		}
		mix(uint64(st.Status))
		mix(uint64(st.ranVer))
		mix(uint64(len(st.Input)))
		mix(uint64(len(st.Output)))
		mix(uint64(len(st.Result)))
		mix(uint64(st.Exit))
		mix(uint64(st.Start.UnixNano()))
		mix(uint64(st.End.UnixNano()))
		mix(uint64(len(st.Children)))
		mix(uint64(len(st.Images)))
		for _, c := range st.Children {
			if !step(c) {
				return false
			}
		}
		return true
	}
	for _, it := range items {
		mix(uint64(it.Kind))
		switch it.Kind {
		case KStep:
			if !step(it.Step) {
				return 0, false
			}
		case KText, KThinking, KInterject, KExchange:
			// Streamed text only grows: its length and how it ends say
			// how far it's got.
			mix(uint64(len(it.Text)))
			for i := max(0, len(it.Text)-8); i < len(it.Text); i++ {
				mix(uint64(it.Text[i]))
			}
			mix(uint64(len(it.Images)))
			if it.Answer {
				mix(1)
			}
		default:
			return 0, false // cheap to draw, and read from what can change
		}
	}
	return h, true
}

// compacted is the line where the conversation was compacted: what set it
// off and how much context went; ctrl+o shows the summary it left.
func (d *drawer) compacted(it *Item) {
	c := it.Compact
	label := paint(cBlue, "◇ context compacted")
	var facts []string
	if c.Trigger != "" {
		facts = append(facts, c.Trigger)
	}
	switch {
	case c.Before > 0 && c.After > 0:
		facts = append(facts, tokens(c.Before)+" → "+tokens(c.After)+" tokens")
	case c.Before > 0:
		facts = append(facts, "from "+tokens(c.Before)+" tokens")
	}
	if it.Text != "" && !d.o.Verbose {
		facts = append(facts, "ctrl+o shows the summary")
	}
	left := d.spine() + "   " + faint("── ") + label
	if len(facts) > 0 {
		left += dim("  " + strings.Join(facts, " · "))
	}
	left += " " + faint(strings.Repeat("─", max(0, d.cw-cellw.String(stripANSI(left))-2)))
	d.add("", "", left, "")
	if d.o.Verbose && it.Text != "" {
		d.prose(it.Text, 6, cDim)
	}
}

// liveLine ends a running turn with what Claude is doing right now, how
// long the turn has run and roughly how much it has written, so a quiet
// stretch (thinking, a long answer being composed) never looks stalled:
// its name and how long on one row, a pulse and the facts under it.
func (d *drawer) liveLine() {
	t := d.t
	if !d.s.compacting.IsZero() {
		d.compactingLine()
		return
	}
	verb, since, waiting := pick(openings, t.Start), t.Start, false
	toolActive := false
	if n := len(t.Items); n > 0 {
		switch last := t.Items[n-1]; {
		case last.Kind == KThinking && !t.Thinking.IsZero():
			verb, since = pick(musings, t.Thinking), t.Thinking
		case last.Kind == KText && d.s.streaming == last:
			verb = pick(writings, t.Start)
		case last.Kind == KStep && (last.Step.Status == Running || d.docked && last.Step.Status == Waiting):
			if !d.docked {
				return // the step's own row is spinning
			}
			toolActive = true
			verb, since = "running "+oneLine(last.Step.Tool), last.Step.Start
			if last.Step.Status == Waiting {
				verb = "waiting for approval"
			}
		case last.Kind == KStep && !last.Step.End.IsZero():
			// Its steps are done and their results sent back: the model
			// is working out what's next, and nothing shows until it says.
			verb, since, waiting = pick(waitings, last.Step.End), last.Step.End, true
		}
	}
	pad := d.spine() + "   "
	d.air()
	head := paint(cOrange+bold, d.spin(d.o.Tick)+" "+verb+"…")
	if !since.IsZero() {
		head += "  " + paint(cOrange, d.since(since))
	}
	var facts []string
	if !t.Start.IsZero() && since != t.Start {
		facts = append(facts, "turn "+d.since(t.Start))
	}
	if t.Streamed > 0 {
		facts = append(facts, "↓ "+tokens(t.Streamed/4)+" tokens")
	}
	if waiting {
		// What it's reading: the context as of the last request, and
		// whether the cache had gone, so all of it is read afresh.
		if d.s.Context > 0 {
			facts = append(facts, "~"+tokens(d.s.Context)+" tokens in")
		}
		if _, cold := d.s.CacheCold(since); cold {
			facts = append(facts, "cache expired, read uncached")
		}
	}
	// What it's done so far, on the same row: the spinner and its clock
	// say it's working; a bar sweeping under them only drew the eye.
	if len(facts) > 0 {
		head += "  " + dim(strings.Join(facts, " · "))
	}
	d.add("", "", pad+head, "")
	if r := t.Retry; r != nil {
		// A request failed and is tried again: the model hasn't stalled,
		// its API has.
		d.add("", "", pad+"  "+paint(cYellow, "↻ "+retryWords(r, d.o.Now.Sub(t.RetryAt))), "")
	} else if quiet := d.o.Now.Sub(latest(t.Heard, t.Start)); !toolActive && !t.Start.IsZero() && quiet >= stallAfter {
		d.add("", "", pad+"  "+paint(cYellow, "⚠ stalled · nothing from the model for "+d.since(d.o.Now.Add(-quiet))+
			" · stop the turn and send again"), "")
	}
}

// stallAfter is how long a turn can hear nothing from its agent, no step
// running, before it's drawn as stalled: a stream gone silent, not thought.
const stallAfter = 2 * time.Minute

// latest is the later of a and b.
func latest(a, b time.Time) time.Time {
	if a.After(b) {
		return a
	}
	return b
}

// air is a row of space above what follows, unless the last row is one.
func (d *drawer) air() {
	if n := len(d.lines); n > 0 && strings.TrimSpace(stripANSI(d.lines[n-1].Text)) == strings.TrimSpace(stripANSI(d.spine())) {
		return
	}
	d.add("", "", d.spine(), "")
}

// glide is a bar's phase, a tenth of a second a step, so it glides rather
// than jumping once a second: it asks for fast frames (Session.Fast).
func (d *drawer) glide() int {
	d.s.Fast = true
	return int(d.o.Now.UnixMilli() / 100)
}

// pick is one of words for the stretch that began at since: the same for
// the whole of it, not a new one each frame.
func pick(words []string, since time.Time) string {
	return words[int(uint64(since.UnixNano())/1e6%uint64(len(words)))]
}

// openings are a turn before the model has said anything; waitings, it
// reading back what its steps did; writings, it composing its answer.
var (
	openings = []string{
		"waiting for the agent", "waking it up", "getting its bearings", "reading the room",
		"finding its feet", "rolling up its sleeves", "clearing its throat", "warming up",
		"pulling up the cuck chair", "stretching its legs", "finding its glasses",
		"switching it off and on again",
	}
	waitings = []string{
		"working out what's next", "reading the results", "chewing it over", "weighing it up",
		"taking stock", "joining the dots", "sizing it up", "having a think", "watching the langs",
		"watching from the cuck chair", "checking its working", "squinting at the output",
		"blaming the compiler", "reading the tea leaves", "nodding knowingly",
	}
	writings = []string{
		"writing", "scribbling", "putting pen to paper", "drafting", "typing away",
		"jotting it down", "spinning a yarn", "writing it up", "licking the pencil",
		"writing its memoirs", "embellishing slightly",
	}
)

// compactingLine is a compaction under way, in the working line's place
// and shape: its name and how long on one row, a run of dots under it.
// Nothing says how far a summary has got, so a time left shows only once
// this session has timed one (compactRate); a guess would only mislead.
func (d *drawer) compactingLine() {
	pad := d.spine() + "   "
	d.air()
	d.add("", "", pad+paint(cOrange+bold, d.spin(d.o.Tick)+" "+compaction(d.s.compacting)+"…")+
		"  "+paint(cOrange, d.since(d.s.compacting)), "")
	frac := -1.0
	var facts []string
	if d.s.Context > 0 {
		facts = append(facts, tokens(d.s.Context)+" tokens to boil down")
		if d.s.compactRate > 0 {
			est := max(5*time.Second, d.s.compactRate*time.Duration(d.s.Context))
			since := d.o.Now.Sub(d.s.compacting)
			frac = min(0.95, float64(since)/float64(est))
			if left := est - since; left > 0 {
				facts = append(facts, "~"+dur(left.Round(time.Second))+" left, as the last one went")
			} else {
				facts = append(facts, "longer than the last one")
			}
		}
	}
	d.add("", "", pad+"  "+dotsBar(frac, 16, d.glide()/2)+"  "+dim(strings.Join(facts, " · ")), "")
}

// dotsBar is w dots, frac of them lit; with nothing to measure (frac < 0)
// a run of three walks along them instead.
func dotsBar(frac float64, w, tick int) string {
	at, fill := tick%(w+3), int(frac*float64(w))
	var b strings.Builder
	for i := range w {
		if frac >= 0 && i < fill || frac < 0 && i <= at && i > at-3 {
			b.WriteString(paint(cOrange, "▰"))
		} else {
			b.WriteString(faint("▱"))
		}
	}
	return b.String()
}

// compactions are what a compaction is called, one for the whole of it:
// in the musings' voice.
var compactions = []string{
	"compacting the context", "boiling it all down", "sitting on the suitcase", "vacuum-packing",
	"writing the cliff notes", "squeezing the sponge", "folding the laundry", "tidying the attic",
	"cramming it in the loft", "taking the minutes", "reducing the stock", "packing it into a flask",
	"rolling up the maps", "wringing it out", "making a précis", "putting it in a nutshell",
}

// compaction is what the compaction that began at since is called.
func compaction(since time.Time) string { return pick(compactions, since) }

// retryWords say a retry: which attempt, why, and when the next goes.
func retryWords(r *event.Retry, since time.Duration) string {
	why := map[int]string{529: "the API is overloaded", 429: "rate limited", 0: "no answer from the API"}[r.Status]
	switch {
	case why != "":
	case r.Status >= 500:
		why = fmt.Sprintf("the API failed (%d)", r.Status)
	default:
		why = firstNonEmpty(r.Err, fmt.Sprintf("error %d", r.Status))
	}
	s := "retrying · " + why + fmt.Sprintf(" · attempt %d", r.Attempt)
	if r.Max > 0 {
		s += fmt.Sprintf(" of %d", r.Max)
	}
	if left := r.Delay - since; left > time.Second {
		s += " · next in " + dur(left.Round(time.Second))
	}
	return s
}

// verb is a step in a word or two, for a folded run: the program a command
// ran, or what a tool did.
func (d *drawer) verb(st *Step) string {
	switch {
	case st.agentRun() != nil:
		return st.agentRun().Name
	case st.fan:
		return "agents"
	case st.kind() == tool.Shell:
		// A chain that committed or pushed is named for that, not its git add.
		if cs := d.stepCards(st); len(cs) > 0 {
			return cs[0].verb()
		}
		cmd := st.in().Command
		switch sh := d.shellShape(cmd); sh.kind {
		case "read", "search", "write":
			return sh.kind
		case "commit":
			return "git commit"
		case "":
			if _, ps := d.phrases(cmd); len(ps) > 0 {
				return ps[0].verb
			}
			return "shell"
		default:
			return strings.Fields(cdRe.ReplaceAllString(strings.TrimSpace(cmd), ""))[0]
		}
	case st.kind() == tool.Read:
		return "read"
	case st.kind() == tool.Search || st.kind() == tool.Glob:
		return "search"
	case st.kind() == tool.Fetch:
		return "fetch"
	case st.kind() == tool.WebSearch:
		return "web search"
	case st.kind() == tool.Subagent:
		return agentName(st)
	case st.Tool == "Skill" || st.Tool == "SlashCommand":
		in := readInput(st.Input)
		return firstNonEmpty(in.str("skill"), in.str("command"), "skill")
	}
	if strings.HasPrefix(st.Tool, "mcp__") {
		if parts := strings.SplitN(strings.TrimPrefix(st.Tool, "mcp__"), "__", 2); len(parts) == 2 {
			return parts[1]
		}
	}
	return st.Tool
}

func (d *drawer) run(ref string, items []*Item) {
	var first, last time.Time
	for _, it := range items {
		if first.IsZero() || it.Step.Start.Before(first) {
			first = it.Step.Start
		}
		if it.Step.End.After(last) {
			last = it.Step.End
		}
	}
	left := d.spine() + blanks(gutter-1) + faint("▸ show "+plural(len(items), "step")+": ") + dim(d.runNames(items))
	left += faint(" · all ok")
	if !first.IsZero() && !last.IsZero() && last.Sub(first) >= 100*time.Millisecond {
		left += faint(" · ") + took(last.Sub(first))
	}
	d.worked = true
	d.add(ref, "", left, "")
}

// runNames is what a run's steps were, by name: "sed ×2, grep, go build".
func (d *drawer) runNames(items []*Item) string {
	var names []string
	for _, g := range d.runGroups(items) {
		names = append(names, g.name)
	}
	return strings.Join(names, ", ")
}

// runGroup is a kind of step in a run, "go test ×2", and whether one of
// them failed.
type runGroup struct {
	name   string
	failed bool
}

// runGroups are a run's kinds of step in the order they came, the first
// four and how many more.
func (d *drawer) runGroups(items []*Item) []runGroup {
	counts := map[string]int{}
	failed := map[string]bool{}
	var order []string
	for _, it := range items {
		g := d.stepMemo(it.Step, 'v', d.verb)
		if counts[g] == 0 {
			order = append(order, g)
		}
		counts[g]++
		failed[g] = failed[g] || it.Step.Status == Failed || d.testsFailed(it.Step)
	}
	var out []runGroup
	for i, g := range order {
		if i == 4 {
			more := false
			for _, h := range order[4:] {
				more = more || failed[h]
			}
			out = append(out, runGroup{fmt.Sprintf("+%d more", len(order)-4), more})
			break
		}
		name := g
		if r, size := utf8.DecodeRuneInString(g); unicode.IsUpper(r) {
			name = string(unicode.ToLower(r)) + g[size:] // "Edit" reads as the rest do
		}
		if n := counts[g]; n > 1 {
			name += fmt.Sprintf(" ×%d", n)
		}
		out = append(out, runGroup{name, failed[g]})
	}
	return out
}

// gap is a blank row, unless the last row already is one, ends a fill, or
// there is none.
func (d *drawer) gap() {
	if n := len(d.lines); n > 0 && !d.isBlank(n-1) {
		d.blank()
	}
}

func (d *drawer) isBlank(i int) bool {
	l := d.lines[i]
	return l.Ref == "" && strings.TrimSpace(stripANSI(l.Text)) == strings.TrimSpace(stripANSI(d.spine()))
}

// prose is Claude's narration: the work axis, secondary colour.
func (d *drawer) prose(s string, indent int, c string) {
	d.markdown(strings.TrimSpace(s), indent, c, false)
}

// answer is the turn's final words: the conversation axis, full text colour,
// with light markdown.
func (d *drawer) answer(s string) {
	// After work, the answer stands a row clear of it.
	d.gap()
	d.markdown(strings.TrimRight(s, " \t\n"), 2, cText, true)
}

// markdown draws text indent columns in, in colour c, with light markdown:
// tables, fenced code, headings and lists. keepBlank keeps its empty lines;
// without it paragraphs close up.
func (d *drawer) markdown(s string, indent int, c string, keepBlank bool) {
	w := min(d.cw-indent-1, capProse)
	pad := d.spine() + blanks(indent-1)
	lines := strings.Split(s, "\n")
	var items []listLevel // the list items open, outermost first
	for li := 0; li < len(lines); li++ {
		trim := strings.TrimSpace(lines[li])
		if strings.HasPrefix(trim, "|") {
			// A markdown table: every row up to the first that isn't one.
			end := li
			for end < len(lines) && strings.HasPrefix(strings.TrimSpace(lines[end]), "|") {
				end++
			}
			if end-li >= 2 {
				// A table stands a row clear of what's around it, even
				// where paragraphs close up.
				d.gap()
				d.table(lines[li:end], pad, min(d.cw-indent-1, d.o.rowCap()), c)
				d.gap()
				li = end - 1
				continue
			}
		}
		if strings.HasPrefix(trim, "```") {
			// A code block: every line to the fence that closes it.
			end := li + 1
			for end < len(lines) && !strings.HasPrefix(strings.TrimSpace(lines[end]), "```") {
				end++
			}
			d.code(lines[li+1:end], strings.TrimSpace(strings.TrimPrefix(trim, "```")), pad)
			li = end
			continue
		}
		switch {
		case trim == "":
			if keepBlank {
				d.blank()
			}
			continue
		case strings.HasPrefix(trim, "#"):
			items = items[:0]
			d.gap() // a heading starts a section, a row clear of the last
			d.add("", "", pad+paint(strong(c), strings.TrimSpace(strings.TrimLeft(trim, "#"))), "")
			continue
		}
		// A list item, or a line of one, sits at its depth: two columns a
		// level, the bullet changing with it, wrapped lines under the text.
		col := indentOf(lines[li])
		marker, body, isItem := listItem(trim)
		lead, ind := marker, ""
		switch {
		case isItem:
			for len(items) > 0 && items[len(items)-1].col >= col {
				items = items[:len(items)-1]
			}
			depth := len(items)
			if marker == "" {
				lead = bullets[depth%len(bullets)]
			}
			items = append(items, listLevel{col: col, text: 2*depth + len([]rune(lead)) + 1})
			ind = blanks(2 * depth)
		case col > 0 && len(items) > 0:
			// Indented under an item: its text goes on there.
			for len(items) > 1 && items[len(items)-1].col >= col {
				items = items[:len(items)-1]
			}
			ind = blanks(items[len(items)-1].text)
		default:
			items = items[:0]
		}
		// A paragraph already drawn this way comes from the memo, so text
		// streaming in only wraps its last paragraph again.
		k := memoKey{text: trim, style: c + ":" + lead, spine: d.spine() + ind, n: indent, width: d.o.Width, cw: d.cw}
		if ls, ok := d.s.memoGet(k); ok {
			d.lines = append(d.lines, ls...)
			continue
		}
		from := len(d.lines)
		if lead != "" {
			lead = dim(lead) + " "
		}
		leadW := len([]rune(stripANSI(lead)))
		for k, r := range wrap(paint(c, inline(body, c)), max(8, w-len(ind)-leadW)) {
			if k > 0 && lead != "" {
				r = blanks(leadW) + r
			} else {
				r = lead + r
			}
			d.add("", "", pad+ind+r, "")
			if k > 0 {
				d.wrapped()
			}
		}
		d.s.memoPut(k, d.lines[from:])
	}
}

// listLevel is an open list item: the column its marker is at in the
// source, and the one its text starts at as drawn.
type listLevel struct{ col, text int }

// bullets mark unordered items, one per depth.
var bullets = []string{"•", "◦", "▪"}

// listItem splits a markdown list item into its marker and text: marker
// is "" for a bullet, the number for an ordered item.
func listItem(trim string) (marker, body string, ok bool) {
	if len(trim) >= 2 && (trim[0] == '-' || trim[0] == '*' || trim[0] == '+') && trim[1] == ' ' {
		return "", strings.TrimSpace(trim[2:]), true
	}
	if m := numbered.FindStringSubmatch(trim); m != nil {
		return m[1], m[2], true
	}
	return "", trim, false
}

// indentOf is a line's leading indent in columns, a tab to the next stop
// of four.
func indentOf(s string) int {
	n := 0
	for _, r := range s {
		switch r {
		case ' ':
			n++
		case '\t':
			n += 4 - n%4
		default:
			return n
		}
	}
	return n
}

// strong is the bold colour for headings drawn in c: white over body text,
// c itself over quieter text.
func strong(c string) string {
	if c == cText {
		return cWhite + bold
	}
	return c + bold
}

// Answer draws text the way a turn's final words are drawn (headings,
// lists, code and tables), w wide, for showing it outside the conversation.
func (s *Session) Answer(text string, w int) []Line {
	d := drawer{s: s, t: &Turn{}, o: Options{Width: w}, cw: min(w, capRow)}
	d.answer(text)
	return d.lines
}

// code draws a fenced code block highlighted in the language its fence
// names (```go); a diff block colours its added and removed lines. A block
// is drawn once and kept while it's in view.
func (d *drawer) code(lines []string, tag, pad string) {
	k := memoKey{text: strings.Join(lines, "\n"), style: "code:" + tag, spine: d.spine(), width: d.o.Width, cw: d.cw}
	if ls, ok := d.s.memoGet(k); ok {
		d.lines = append(d.lines, ls...)
		return
	}
	from := len(d.lines)
	defer d.widen(lines, 7)()
	w := d.cw - 7
	lg := langFor(tag)
	if f := strings.Fields(tag); lg == nil && len(f) > 0 {
		lg = langFor(f[0]) // ```go title="x.go"
	}
	isDiff := tag == "diff" || tag == "patch"
	var ud *udiff
	if isDiff {
		ud = parseDiff(lines, func(int) bool { return true })
	}
	var st hlState
	for i, l := range lines {
		if ud != nil && ud.line(d, pad+" ", w, nil, lines, i) {
			continue
		}
		l = expandTabs(l)
		switch {
		case isDiff && strings.HasPrefix(l, "+") && !strings.HasPrefix(l, "+++"):
			d.addRows(bgAdd, pad+" ", plusSign(), text(l[1:]), w-1, 0)
		case isDiff && strings.HasPrefix(l, "-") && !strings.HasPrefix(l, "---"):
			d.addRows(bgDel, pad+" ", minusSign(), text(l[1:]), w-1, 0)
		case isDiff && strings.HasPrefix(l, "@@"):
			d.addRows(bgWell, pad+" ", "", paint(cBlue, l), w, 0)
		default:
			d.addRows(bgWell, pad+" ", "", highlight(lg, &st, l, cSub, nil), w, 0)
		}
	}
	d.s.memoPut(k, d.lines[from:])
}

var tableSep = regexp.MustCompile(`^:?-{2,}:?$`)

// Tables use compact columns; short mappings become labelled lines.
// Cells wrap within the available width without per-row borders.
func (d *drawer) table(rows []string, pad string, w int, col string) {
	var cells [][]string
	head := -1
	for _, r := range rows {
		r = strings.TrimSpace(r)
		r = strings.TrimSuffix(strings.TrimPrefix(r, "|"), "|")
		parts := strings.Split(r, "|")
		sep := true
		for i := range parts {
			parts[i] = strings.TrimSpace(parts[i])
			if !tableSep.MatchString(parts[i]) {
				sep = false
			}
		}
		if sep {
			head = len(cells) - 1
			continue
		}
		cells = append(cells, parts)
	}
	cols := 0
	for _, r := range cells {
		cols = max(cols, len(r))
	}
	if cols == 0 {
		return
	}
	// Short two-column mappings read more naturally as a compact list.
	if cols == 2 && len(cells) <= 5 {
		for ri, cells := range cells {
			key := mdMarks.Replace(cells[0])
			value := ""
			if len(cells) > 1 {
				value = mdMarks.Replace(cells[1])
			}
			// The words' own colours: blue reads as a link.
			line := paint(strong(col), key) + faint(" · ") + paint(col, value)
			if ri == head {
				line = paint(cSub+bold, key) + faint(" · ") + paint(cSub+bold, value)
			}
			for _, part := range wrap(line, max(1, w)) {
				d.add("", "", pad+part, "")
			}
		}
		return
	}
	// When even one character per column cannot fit, retain every cell as
	// wrapped text instead of clipping columns off the terminal edge.
	if 3*cols-2 > w {
		for ri, cells := range cells {
			ink := col
			if ri == head {
				ink = cSub + bold
			}
			line := paint(ink, mdMarks.Replace(strings.Join(cells, " · ")))
			for _, part := range wrap(line, max(1, w)) {
				d.add("", "", pad+part, "")
			}
		}
		return
	}
	width := make([]int, cols)
	for _, r := range cells {
		for i, c := range r {
			width[i] = max(width[i], cellw.String(mdMarks.Replace(c)))
		}
	}
	chrome := 2 * (cols - 1)
	for total := sum(width) + chrome; total > w; total = sum(width) + chrome {
		i := 0
		for j := range width {
			if width[j] > width[i] {
				i = j
			}
		}
		if width[i] <= 1 {
			break
		}
		width[i]--
	}
	for ri, r := range cells {
		parts := make([][]string, cols)
		height := 1
		for i := range cols {
			value := ""
			if i < len(r) {
				value = mdMarks.Replace(r[i])
			}
			parts[i] = strings.Split(strings.TrimRight(ansi.Wrap(value, max(1, width[i]), ""), "\n"), "\n")
			height = max(height, len(parts[i]))
		}
		for line := range height {
			var b strings.Builder
			for i := range cols {
				value := ""
				if line < len(parts[i]) {
					value = strings.TrimRight(parts[i][line], " ")
				}
				ink := col
				if ri == head {
					ink = cSub + bold
				} else if i == 0 {
					ink = cSub + bold
				}
				b.WriteString(paint(ink, value))
				if i < cols-1 {
					b.WriteString(blanks(max(0, width[i]-cellw.String(value)) + 2))
				}
			}
			d.add("", "", pad+b.String(), "")
		}
	}

}

func sum(xs []int) int {
	n := 0
	for _, x := range xs {
		n += x
	}
	return n
}

// memoKey names a paragraph as drawn: its text, how, and at what width.
type memoKey struct {
	text, style, spine string
	n, width, cw, pal  int
}

// memoTurn ages the memos. Each keeps what it's asked for until it holds
// memoKeep entries; then what was asked for since the last time stays and
// the rest goes. Ageing every render would drop what a memoized unit drew
// without asking: a folded run that gains a step would work out every
// step in it again, in one frame.
func (s *Session) memoTurn() {
	if f := strings.Join(s.bases(), "|"); f != s.rowsFor {
		s.rows, s.rowsOld, s.rowsFor = nil, nil, f // paths read relative to other folders now
	}
	s.memoOld, s.memo = age(s.memoOld, s.memo)
	s.chainsOld, s.chains = age(s.chainsOld, s.chains)
	s.rowsOld, s.rows = age(s.rowsOld, s.rows)
	s.cardsOld, s.cards = age(s.cardsOld, s.cards)
}

const memoKeep = 8192

func age[K comparable, V any](old, cur map[K]V) (map[K]V, map[K]V) {
	switch {
	case cur == nil:
		return old, make(map[K]V)
	case len(cur) < memoKeep:
		return old, cur
	}
	return cur, make(map[K]V, len(cur)/2)
}

// stepKey names a step's label, summary or verb as drawn: they read only
// the step, so they hold until its status, output or subagent steps change.
// A running turn redraws every second; its finished steps don't need to
// parse their input and output again each time.
type stepKey struct {
	st                  *Step
	what                byte
	status              Status
	out, res, kids, pal int
	ran                 int // how often the agents it ran were set
}

func (d *drawer) stepMemo(st *Step, what byte, f func(*Step) string) string {
	s := d.s
	k := stepKey{st: st, what: what, status: st.Status, out: len(st.Output), res: len(st.Result), kids: len(st.Children), pal: palette, ran: st.ranVer}
	if v, ok := s.rows[k]; ok {
		return v
	}
	v, ok := s.rowsOld[k]
	if !ok {
		v = f(st)
	}
	if s.rows != nil {
		s.rows[k] = v
	}
	return v
}

// A memo is of a drawing in the palette of the time: memoGet and memoPut
// key it by that.
func (s *Session) memoGet(k memoKey) ([]Line, bool) {
	k.pal = palette
	if ls, ok := s.memo[k]; ok {
		return ls, true
	}
	if ls, ok := s.memoOld[k]; ok && s.memo != nil {
		s.memo[k] = ls
		return ls, true
	}
	return nil, false
}

func (s *Session) memoPut(k memoKey, ls []Line) {
	k.pal = palette
	if s.memo != nil {
		s.memo[k] = append([]Line(nil), ls...)
	}
}

const spaces = "                                                                                                                                "

// blanks is n spaces.
func blanks(n int) string {
	if n <= 0 {
		return ""
	}
	if n <= len(spaces) {
		return spaces[:n]
	}
	return strings.Repeat(" ", n)
}

// styledAsk draws your words with the things that act singled out: a
// shell command tinted, slash commands and @files in bold white, image and
// paste markers as chips, links underlined.
func styledAsk(s, base string) string {
	if cmd, ok := strings.CutPrefix(s, "! "); ok {
		return paint(cWhite+bold, "$ ") + tint(cmd)
	}
	s = specialRe.ReplaceAllStringFunc(s, func(m string) string {
		switch {
		case strings.HasPrefix(m, "[image: "):
			return reset + paint(cBlue, "▣ ") + paint(cText, ImageLabel(strings.TrimSuffix(m[len("[image: "):], "]"))) + base
		case strings.HasPrefix(m, "[Image"):
			return reset + paint(cBlue, "▣ ") + paint(cText, strings.Trim(m, "[]")) + base
		case strings.HasPrefix(m, "[Pasted"), strings.HasPrefix(m, "[pasted"), strings.HasPrefix(m, "[#"):
			return reset + paint(cBlue, "▤ ") + paint(cText, strings.Trim(m, "[]")) + base
		case strings.HasPrefix(m, "http"):
			return m // inline links it, once: linked here too, it'd link the link
		}
		return reset + paint(cWhite+bold, m) + base
	})
	return paint(base, inline(s, base))
}

var specialRe = regexp.MustCompile(`\[Image #\d+\]|\[image: [^\]]+\]|\[(?:[Pp]asted text )?#\d+ [^\]\n]*lines?[^\]\n]*\]|https?://[^\s)>\]]+|(^|\s)/[a-z][\w:-]*(?:$|[\s.,;:!?)])|@[\w./-]+`)

// URLIn is the first link in s, whoever's it is; "" when there's none.
func URLIn(s string) string {
	i := strings.Index(s, "http")
	for ; i >= 0; i = strings.Index(s, "http") {
		j := i + 4
		switch {
		case strings.HasPrefix(s[j:], "s://"):
			j += 4
		case strings.HasPrefix(s[j:], "://"):
			j += 3
		default:
			s = s[i+4:]
			continue
		}
		if n := urlLen(s[j:]); n > 0 {
			return s[i : j+n]
		}
		s = s[j:]
	}
	return ""
}

var pastedRe = regexp.MustCompile(`(?s)\s*<pasted_content(?: id="[^"]*")?>\n?(.*?)\n?</pasted_content(?: id="[^"]*")?>\s*`)

// EachPaste replaces each paste Claude Code marks in a message, the text
// between <pasted_content> tags, with what f makes of it.
func EachPaste(s string, f func(text string) string) string {
	if !strings.Contains(s, "<pasted_content") {
		return s
	}
	return pastedRe.ReplaceAllStringFunc(s, func(m string) string {
		return " " + f(pastedRe.FindStringSubmatch(m)[1]) + " "
	})
}

// FoldPastes shows each paste in a message as the chip it was in the box.
func FoldPastes(s string) string {
	n := 0
	return strings.TrimSpace(EachPaste(s, func(text string) string {
		n++
		return PasteChip(n, text)
	}))
}

// link underlines a URL and makes it clickable in terminals that support
// OSC 8 hyperlinks.
func link(url string) string {
	return "\x1b]8;;" + url + "\x1b\\" + paint(cBlue+"\x1b[4m", url) + "\x1b]8;;\x1b\\"
}

// Inline styles **bold** and `code` in s and links its URLs, returning to
// base colour after each.
func Inline(s, base string) string { return inline(s, base) }

var numbered = regexp.MustCompile(`^(\d+[.)])\s+(.*)$`)

// inline styles **bold** and `code`, returning to base colour after each,
// draws markdown images and links, and links URLs. Byte scans, the same as
// replacing \*\*([^*]+)\*\*, `([^`]+)` and https?://[^\s)>\]"'`]+ in turn.
func inline(s, base string) string {
	if !strings.ContainsAny(s, "*`h[") {
		return s
	}
	s = pairs(s, "**", bold, reset+base)
	// Code stands one shade above its words: white in the answer, text
	// colour in quieter narration, so it never outshines the answer.
	code := cWhite
	if base == cSub || base == cDim {
		code = cText
	}
	s = pairs(s, "`", code, reset+base)
	return mdLinks(s, base)
}

// mdLinks draws each markdown image, ![alt](src), as a chip naming it and
// each [text](url) as its text, both opening what they point at when
// clicked, and links the bare URLs between them.
func mdLinks(s, base string) string {
	if !strings.Contains(s, "](") {
		return links(s, base)
	}
	var b strings.Builder
	last := 0
	for p := 0; ; {
		i := strings.Index(s[p:], "](")
		if i < 0 {
			break
		}
		mid := p + i
		open := strings.LastIndexByte(s[last:mid], '[')
		end := strings.IndexByte(s[mid+2:], ')')
		if open < 0 || end < 0 {
			p = mid + 2
			continue
		}
		open += last
		end += mid + 2
		label, target := s[open+1:mid], strings.TrimSpace(s[mid+2:end])
		if strings.ContainsAny(label, "[]") || target == "" || strings.ContainsAny(target, " \t") {
			p = mid + 2
			continue
		}
		image := open > last && s[open-1] == '!'
		start := open
		if image {
			start--
		}
		b.WriteString(links(s[last:start], base))
		b.WriteString(reset)
		if image {
			b.WriteString(imageChip(label, target))
		} else if u := linkTarget(target); u != "" {
			b.WriteString("\x1b]8;;" + u + "\x1b\\" + paint(cBlue+"\x1b[4m", label) + "\x1b]8;;\x1b\\")
		} else {
			b.WriteString(paint(cBlue, label))
		}
		b.WriteString(base)
		last, p = end+1, end+1
	}
	if last == 0 {
		return links(s, base)
	}
	b.WriteString(links(s[last:], base))
	return b.String()
}

// imageChip draws an image Claude points at as the prompt draws one sent
// with it, ▣ and its name, with its file after, opening it when clicked.
func imageChip(alt, src string) string {
	name := path.Base(src)
	if alt == "" {
		alt = name
	}
	chip := paint(cBlue, "▣ ") + paint(cText, alt)
	if name != alt {
		chip += paint(cDim, " "+name)
	}
	if u := linkTarget(src); u != "" {
		return "\x1b]8;;" + u + "\x1b\\" + chip + "\x1b]8;;\x1b\\"
	}
	return chip
}

// fileLink makes s open the file at p when clicked, a relative p taken
// from the session's folder; s as it is when there's no telling where p is.
func (d *drawer) fileLink(p, s string) string {
	if u := linkTarget(d.abs(p)); u != "" {
		return "\x1b]8;;" + u + "\x1b\\" + s + "\x1b]8;;\x1b\\"
	}
	return s
}

// abs is p from the session's folder when it's relative; nothing when
// there's no folder to take it from.
func (d *drawer) abs(p string) string { return d.s.abs(p) }

func (s *Session) abs(p string) string {
	if p == "" || filepath.IsAbs(p) || strings.HasPrefix(p, "~/") {
		return p
	}
	base := firstNonEmpty(s.Info.Cwd, s.Cwd)
	if base == "" {
		return ""
	}
	return filepath.Join(base, p)
}

// linkTarget is the URL a markdown link or image opens: web addresses as
// they are, absolute paths and ~ as file URLs, nothing for the rest.
func linkTarget(t string) string {
	switch {
	case strings.HasPrefix(t, "http://"), strings.HasPrefix(t, "https://"), strings.HasPrefix(t, "file://"):
		return t
	case strings.HasPrefix(t, "~/"):
		home, err := os.UserHomeDir()
		if err != nil {
			return ""
		}
		t = filepath.Join(home, t[2:])
	}
	if !filepath.IsAbs(t) {
		return ""
	}
	return (&url.URL{Scheme: "file", Path: t}).String()
}

// pairs wraps text between two delimiters in open and close, dropping the
// delimiters, where the text is at least one character and holds no
// delimiter character.
func pairs(s, delim, open, close string) string {
	d := delim[0]
	var b strings.Builder
	last := 0
	for p := 0; ; {
		i := strings.Index(s[p:], delim)
		if i < 0 {
			break
		}
		i += p
		from := i + len(delim)
		j := strings.IndexByte(s[from:], d)
		if j <= 0 || !strings.HasPrefix(s[from+j:], delim) {
			p = i + 1
			continue
		}
		if b.Len() == 0 {
			b.Grow(len(s) + 32)
		}
		b.WriteString(s[last:i])
		b.WriteString(open)
		b.WriteString(s[from : from+j])
		b.WriteString(close)
		last = from + j + len(delim)
		p = last
	}
	if last == 0 {
		return s
	}
	b.WriteString(s[last:])
	return b.String()
}

// links makes each URL a link, returning to base colour after it.
func links(s, base string) string {
	var b strings.Builder
	last := 0
	for p := 0; ; {
		i := strings.Index(s[p:], "http")
		if i < 0 {
			break
		}
		i += p
		j := i + 4
		if j < len(s) && s[j] == 's' && strings.HasPrefix(s[j+1:], "://") {
			j += 4
		} else if strings.HasPrefix(s[j:], "://") {
			j += 3
		} else {
			p = i + 1
			continue
		}
		end := j + urlLen(s[j:])
		if end == j {
			p = i + 1
			continue
		}
		b.WriteString(s[last:i])
		b.WriteString(reset)
		b.WriteString(link(s[i:end]))
		b.WriteString(base)
		last, p = end, end
	}
	if last == 0 {
		return s
	}
	b.WriteString(s[last:])
	return b.String()
}

// trimURL drops the punctuation that ends the sentence a URL closes, so
// "see https://x.dev." links https://x.dev.
func trimURL(u string) string {
	return strings.TrimRight(u, ".:;!?")
}

// urlLen is how much of s, just past a URL's scheme, is the URL: up to a
// space or quote, or a ) or > the URL didn't open itself, so a placeholder
// like http://127.0.0.1:<port> or a wiki's _(film) stays in, and
// <https://x.dev> or (see https://x.dev) keeps its bracket out.
func urlLen(s string) int {
	angle, paren, end := 0, 0, 0
	for ; end < len(s) && !urlStop(s[end]); end++ {
		switch s[end] {
		case '<':
			angle++
		case '(':
			paren++
		}
		if s[end] == '>' {
			if angle == 0 {
				break
			}
			angle--
		}
		if s[end] == ')' {
			if paren == 0 {
				break
			}
			paren--
		}
	}
	return len(trimURL(s[:end]))
}

func urlStop(c byte) bool {
	switch c {
	case '\t', '\n', '\f', '\r', ' ', ']', '"', '\'', '`', ',':
		return true
	}
	return false
}

// stripANSI drops colour codes (ESC [ digits and semicolons m), as ansiRe
// would, with a byte scan.
// cleanOutput takes every escape sequence and control character out of a
// line of a tool's output, keeping tabs for expandTabs.
func cleanOutput(s string) string {
	if !strings.ContainsFunc(s, func(r rune) bool { return r < 0x20 && r != '\t' || r == 0x7f }) {
		return s
	}
	s = ansi.Strip(s)
	return strings.Map(func(r rune) rune {
		if r < 0x20 && r != '\t' || r == 0x7f {
			return -1
		}
		return r
	}, s)
}

// unscreen undoes a screen a program drew for its own terminal's size
// (a TUI, an editor, a test printing one): padding to that width, gaps
// pushing text to its right edge, an editor's ~ rows past the end of a
// file, runs of blank rows. Drawn at another width, those wrap into blocks
// and shapes.
// ponytail: a 24-space gap reads as screen padding, so a table with wider
// columns than that closes up; key it on the line's width if one does.
func unscreen(s string) string {
	lines := strings.Split(s, "\n")
	out := lines[:0]
	blank, tilde := 0, 0
	for _, l := range lines {
		l = strings.TrimRight(l, " \t")
		if t := strings.TrimSpace(l); t != "" && t != "~" {
			if i := strings.IndexFunc(l, func(r rune) bool { return r != ' ' }); i >= 0 {
				l = l[:i] + screenGap.ReplaceAllString(l[i:], "   ")
			}
		}
		switch strings.TrimSpace(l) {
		case "":
			if blank++; blank > 1 {
				continue
			}
		case "~":
			if tilde++; tilde > 1 {
				continue
			}
		default:
			blank, tilde = 0, 0
		}
		out = append(out, l)
	}
	return strings.Join(out, "\n")
}

var screenGap = regexp.MustCompile(` {24,}`)

func stripANSI(s string) string {
	i := strings.IndexByte(s, 0x1b)
	if i < 0 {
		return s
	}
	var b strings.Builder
	b.Grow(len(s))
	for i >= 0 {
		b.WriteString(s[:i])
		j := i + 1
		if j < len(s) && s[j] == '[' {
			j++
			for j < len(s) && (s[j] >= '0' && s[j] <= '9' || s[j] == ';') {
				j++
			}
			if j < len(s) && s[j] == 'm' {
				s = s[j+1:]
				i = strings.IndexByte(s, 0x1b)
				continue
			}
		}
		b.WriteByte(0x1b)
		s = s[i+1:]
		i = strings.IndexByte(s, 0x1b)
	}
	b.WriteString(s)
	return b.String()
}

// foldable steps are the clean, routine ones a finished turn can fold into
// a single row. Edits never fold: they're what you'd review; nor do
// pictures, which show on their own.
func foldable(st *Step) bool {
	if st.Status != OK || hidden(st) || len(st.Images) > 0 || st.kind() == tool.Read && thumbable(st.in().Path) {
		return false
	}
	switch glyphFor(st) {
	case "✎", "⇉", "◆", "◇":
		return false
	}
	return !messageTool(st.Tool)
}

// hidden steps are bookkeeping the task line already shows.
func hidden(st *Step) bool {
	switch {
	case st.kind() == tool.Todo || st.Tool == "TaskCreate" || st.Tool == "TaskUpdate" || st.Tool == "TaskList" || st.Tool == "TaskGet":
		return true
	}
	return false
}

func (d *drawer) statusMark(st *Step) string {
	status := st.Status
	if st.runLive && status == OK {
		status = Running // its shell returned; the agent it ran works on
	}
	switch status {
	case Running:
		return paint(cOrange, d.spin(d.o.Tick+len(st.ID)))
	case OK:
		if d.testsFailed(st) {
			return paint(cRed, "✗")
		}
		return dim("✓")
	case Failed:
		return paint(cRed, "✗")
	case Waiting:
		return paint(cYellow, "●")
	case Denied:
		if _, ok := classified(st); ok {
			return paint(cYellow, "⊘")
		}
		return dim("⊘")
	default:
		return dim("◌")
	}
}

// stepOpen is whether a step draws what it ran and did under its row: the
// latest does, unless a card says what it did, or you opened or closed it.
func (d *drawer) stepOpen(st *Step, ref string) bool {
	if v, ok := d.o.Open[ref]; ok {
		return v
	}
	return d.o.Verbose || st == d.latest && len(d.stepCards(st)) == 0
}

func (d *drawer) step(st *Step, depth int) {
	if hidden(st) {
		return
	}
	ref := d.ref + ":s:" + st.ID
	indent := gutter + depth*2
	// The agents' reply, read from their task's output, is theirs to say.
	if !d.o.Verbose && d.replyOf(st) {
		return
	}
	if st.Tool == agtools.Show && st.Status != Failed && d.figure(st, ref, indent) {
		return
	}
	if d.skillStep(st, ref, indent) {
		return
	}
	if messageTool(st.Tool) {
		d.message(st, ref, indent)
		return
	}
	// How it came out follows the label, so the eye never has to cross the
	// pane for it; the label gives way first when the row is too long.
	lead := d.spine() + strings.Repeat(" ", indent-1) + d.statusMark(st) + " "
	label := d.stepMemo(st, 'l', d.label)
	cells, right := d.cells(st)
	if d.o.Paused[st.ID] && st.Status == Running {
		cells = strings.TrimPrefix(cells+faint(" · ")+paint(cYellow+bold, "⏸ paused"), faint(" · "))
	}
	if cells != "" {
		cells = faint("  · ") + cells
		room := d.cw - cellw.String(lead) - cellw.String(cells) - cellw.String(right) - 3
		if room >= 12 && cellw.String(label) > room {
			label = cellw.Truncate(label, room, "…")
		}
	}
	left := lead + label + cells
	d.worked = true
	open := d.stepOpen(st, ref)
	// A failure shows just its error until you open it for everything.
	background := ""
	if st.Status == Failed || d.testsFailed(st) {
		background = bgFailure
	}
	brief := st.Status == Failed && !open
	if brief {
		d.add(ref, background, left, right)
		// A card says what went wrong better than a line of the output.
		if len(d.stepCards(st)) == 0 {
			d.errorLine(st, indent+4, ref)
		}
	} else {
		hint := ""
		if open {
			hint = d.viewHint(st, ref)
		}
		if hint != "" && right != "" {
			hint += faint(" · ")
		}
		d.add(ref, background, left, hint+right)
	}
	// What it did comes after what it ran, when that's open. A picture it
	// read shows either way, as a card would.
	if open {
		d.body(st, indent+4)
	} else if !brief {
		d.pictures(st, indent+4)
	}
	d.cards(st, indent+2)
	d.denial(st, indent+2)
	// A subagent shows its own steps while it works, or when opened: only
	// its latest few until it's opened, as its runs go long. The agents a
	// command ran show always, every one.
	if len(st.Children) > 0 && (st.Status == Running || st.runLive || open || st.fan) {
		kids := st.Children
		// The rows under a step that ran them hang on a rail from its glyph.
		d.railed(indent, false, func() {
			// Open only for being the latest step isn't opened.
			if !d.o.Verbose && !d.o.Open[ref] && !st.fan && len(kids) > spawnShown {
				d.add("", "", d.spine()+blanks(indent+1)+faint(fmt.Sprintf("⋯ %s before", plural(len(kids)-spawnShown, "step"))), "")
				kids = kids[len(kids)-spawnShown:]
			}
			for _, c := range kids {
				// Each a unit of its own: a working subagent's done steps are
				// drawn once, not every frame its parent's clock ticks.
				if c.unit == nil {
					c.unit = []*Item{{Kind: KStep, Step: c}}
				}
				d.memoized(c.unit, "", true, func() { d.step(c, depth+1) })
			}
		})
	}
}

// cells is what a step's row says after its label, and on the right how
// long it took, dim.
func (d *drawer) cells(st *Step) (string, string) {
	var parts []string
	if st.Status == Waiting {
		wait := ""
		if !st.Start.IsZero() {
			wait = dim(d.since(st.Start)) + "  "
		}
		return paint(cYellow, "waiting on you"), wait
	}
	if blocked(st) {
		return paint(cRed, "blocked by the harness"), ""
	}
	// A card under the row says it better.
	if s := d.stepMemo(st, 's', d.summary); s != "" && len(d.stepCards(st)) == 0 {
		parts = append(parts, s)
	}
	if st.kind() == tool.Shell && st.Exit > 0 {
		switch st.Exit {
		case 124:
			parts = append(parts, paint(cRed, "timed out"))
		case 137, 143:
			parts = append(parts, paint(cRed, "killed"))
		default:
			parts = append(parts, paint(cRed, fmt.Sprintf("exit %d", st.Exit)))
		}
	}
	end, right := st.End, ""
	if st.runEnd.After(end) {
		end = st.runEnd
	}
	switch {
	case (st.Status == Running || st.runLive) && !st.Start.IsZero():
		if p := st.runningPart(); p != "" {
			parts = append(parts, paint(cSub, p))
		}
		right = dim(d.since(st.Start))
		// One running a while says when it started, to tell stuck from slow
		// against the clock.
		if d.o.Now.Sub(st.Start) >= 30*time.Second {
			right += faint(" since " + st.Start.Local().Format("15:04"))
		}
	case !end.IsZero() && !st.Start.IsZero() && end.Sub(st.Start) >= 100*time.Millisecond:
		right = took(end.Sub(st.Start))
	}
	if right != "" {
		right += "  "
	}
	return strings.Join(parts, faint(" · ")), right
}

// took is how long something finished took: dim, whatever it was.
func took(d time.Duration) string { return dim(dur(d)) }

// --- labels ---

type input map[string]any

func (in input) str(k string) string {
	v, _ := in[k].(string)
	return v
}

func readInput(raw jsontext.Value) input {
	var m input
	_ = jsonx.Unmarshal(raw, &m)
	return m
}

// agentName is who a Task or Agent call ran: the agent Claude Code reports
// in its result (it resolves an omitted or aliased subagent_type), else the
// one asked for, else just "subagent".
func agentName(st *Step) string {
	n := len(st.Result)
	if n > 0 && st.agentFor == n { // the Overview asks of every run, every frame
		return st.agent
	}
	var r struct {
		AgentType string `json:"agentType"`
	}
	name := firstNonEmpty(st.in().Agent, "subagent")
	if n > 0 && jsonx.Unmarshal(st.Result, &r) == nil && r.AgentType != "" {
		name = r.AgentType
	}
	if n > 0 {
		st.agent, st.agentFor = name, n
	}
	return name
}

func glyphFor(st *Step) string {
	switch {
	case st.ranAgents() || st.toolRun() != nil:
		return "⇉"
	case st.kind() == tool.Shell:
		return "$"
	case st.kind() == tool.Edit || st.kind() == tool.Write || st.kind() == tool.Notebook || st.kind() == tool.Delete || st.kind() == tool.Move:
		return "✎"
	case st.kind() == tool.Read:
		return "◧"
	case st.kind() == tool.Search || st.kind() == tool.Glob:
		return "⌕"
	case st.kind() == tool.Subagent:
		return "⇉"
	case st.kind() == tool.Fetch || st.kind() == tool.WebSearch:
		return "↗"
	case st.Tool == "Artifact":
		return "◆"
	case st.Tool == "Skill" || st.Tool == "SlashCommand":
		return "✦"
	case st.Tool == agtools.Show:
		return "◇"
	}
	return "•"
}

// Tool categories use a small accent; ordinary shell output stays neutral.
func glyphColor(g string) string {
	switch g {
	case "◧", "⇉", "↗":
		return paint(cBlue, g)
	case "⌕":
		return paint(cYellow, g)
	case "✎", "✦":
		return paint(cOrange, g)
	}
	return faint(g)
}

// since is how long ago t was, for a timer still running; one under
// fastUnder asks for frames more often than every second (Session.Fast).
func (d *drawer) since(t time.Time) string {
	x := d.o.Now.Sub(t)
	if x < fastUnder {
		d.s.Fast = true
	}
	return dur(x)
}

// rel shortens a path to the session's folder when it's inside it, looking
// through symlinks such as macOS's /tmp → /private/tmp.
func (d *drawer) rel(p string) string {
	for _, base := range d.s.bases() {
		if r, err := filepath.Rel(base, p); err == nil && !strings.HasPrefix(r, "..") {
			return r
		}
	}
	return shortAbs(p)
}

// shortAbs is a path outside the session as it reads best: home as ~, and
// the middle of a long one left out so its file's name still shows.
func shortAbs(p string) string {
	if h, err := os.UserHomeDir(); err == nil && h != "" && strings.HasPrefix(p, h+"/") {
		p = "~" + p[len(h):]
	}
	parts := strings.Split(p, "/")
	if len(p) <= 48 || len(parts) <= 4 {
		return p
	}
	return strings.Join(append(parts[:2:2], append([]string{"…"}, parts[len(parts)-2:]...)...), "/")
}

func (d *drawer) label(st *Step) string {
	x := st.in()
	g := glyphColor(glyphFor(st))
	// A step is the log's quiet voice; one still going, waiting or failed
	// reads a shade up.
	base := cDim
	if st.Status != OK {
		base = cSub
	}
	lbl := func(s string) string { return paint(base, s) }
	switch {
	case st.kind() == tool.Shell || st.toolRun() != nil:
		if st.run != nil {
			return spawnLabel(*st.run, oneLine(x.Description), lbl)
		}
		if sp := st.toolRun(); sp != nil {
			named := *sp
			if named.Model != "" {
				named.Name += " · " + agent.ModelName(named.Kind, named.Model)
			}
			return spawnLabel(named, firstLine(sp.Prompt), lbl)
		}
		// Several agents it ran are rows of their own, under what it's for.
		if st.fan {
			return glyphColor("⇉") + " " + lbl(firstNonEmpty(oneLine(x.Description), "ran agents"))
		}
		cmd := x.Command
		// What the command is for reads faster than the command; the command
		// itself follows, quieter, and shows whole when the row is opened.
		if desc := oneLine(x.Description); desc != "" {
			return g + " " + lbl(desc) + "  " + faint(program(cmd))
		}
		// A read, a search or a write says so, as the tool it stands in for.
		if sh := d.shellShape(cmd); sh.kind != "" {
			l := glyphColor(sh.glyph) + " "
			if sh.in != "" {
				l += faint("in " + sh.in + " · ")
			}
			return l + lbl(sh.what)
		}
		// A chain, or a command too long to read at a glance, says what
		// each of its commands does.
		if l := d.chain(cmd, base); l != "" {
			return l
		}
		return g + " " + d.command(cmd, base)
	case st.kind() == tool.Edit || st.kind() == tool.Write || st.kind() == tool.Notebook || st.kind() == tool.Delete || st.kind() == tool.Move:
		return g + " " + d.fileLink(x.Path, lbl(d.rel(x.Path)))
	case st.kind() == tool.Read:
		return g + " " + d.fileLink(x.Path, lbl(d.rel(x.Path)))
	case st.kind() == tool.Search:
		l := g + " " + lbl(x.Pattern)
		if p := x.Path; p != "" {
			l += faint(" in ") + lbl(d.rel(p))
		}
		return l
	case st.kind() == tool.Glob:
		return g + " " + lbl(x.Pattern)
	case st.kind() == tool.Subagent:
		return g + " " + lbl(agentName(st)) + "  " + faint(oneLine(x.Description))
	case st.kind() == tool.Fetch:
		return g + " " + lbl(x.URL)
	case st.kind() == tool.WebSearch:
		return g + " " + lbl(x.Query)
	case st.kind() == tool.Question:
		return paint(cYellow, "?") + " " + text(oneLine(st.Call().Title))
	}
	// The rest are drawn by name, from their input as their agent sent it:
	// Claude Code's own tools (Skill, Show, Artifact) and anything else.
	in := readInput(st.Input)
	switch st.Tool {
	case "Skill", "SlashCommand":
		name := firstNonEmpty(in.str("skill"), in.str("command"), in.str("name"))
		return g + " " + lbl(name) + "  " + faint(oneLine(in.str("args")))
	case agtools.Show:
		return g + " " + lbl(firstNonEmpty(oneLine(in.str("title")), "drawing"))
	case "Artifact":
		t := in.str("title")
		if t == "" {
			t = filepath.Base(in.str("file_path"))
		}
		// Where it was published, so the link isn't lost in the output.
		if u := URLIn(st.Output); u != "" {
			return g + " " + lbl(t) + "  " + faint(u)
		}
		return g + " " + lbl(t)
	}
	if l, ok := d.toolLabel(st, lbl); ok {
		return l
	}
	name := st.Tool
	if strings.HasPrefix(name, "mcp__") {
		parts := strings.SplitN(strings.TrimPrefix(name, "mcp__"), "__", 2)
		if len(parts) == 2 {
			name = parts[1] + faint(" · "+parts[0])
		}
	}
	return g + " " + lbl(name) + "  " + faint(firstValue(in))
}

func firstValue(in input) string {
	for _, k := range []string{"description", "prompt", "query", "path", "url", "name", "skill"} {
		if v := in.str(k); v != "" {
			return oneLine(v)
		}
	}
	return ""
}

var cdRe = regexp.MustCompile(`^cd\s+("[^"]+"|'[^']+'|\S+)\s*&&\s*`)

// command is a shell command's first line in the row's colour c: a cd into
// another folder leads, quieter, and a longer command says how much more.
func (d *drawer) command(cmd, c string) string {
	cmd = strings.TrimSpace(cmd)
	// Paths inside the session's folder read better relative to it.
	for _, base := range d.s.bases() {
		cmd = strings.ReplaceAll(cmd, base+"/", "")
	}
	lines := strings.Split(cmd, "\n")
	first := lines[0]
	lead := ""
	if m := cdRe.FindStringSubmatch(first); m != nil {
		dir := strings.Trim(m[1], `"'`)
		first = first[len(m[0]):]
		if dir != d.s.Info.Cwd && dir != "." {
			lead = faint("in " + d.rel(dir) + " · ")
		}
	}
	out := lead + paint(c, first)
	if n := len(lines) - 1; n > 0 {
		out += faint(fmt.Sprintf("  +%d lines", n))
	}
	return out
}

// program is a command's first word, or first two for git, go, npm and the
// like, so a row can say "git status" without the whole pipeline.
func program(cmd string) string {
	cmd = strings.TrimSpace(cdRe.ReplaceAllString(strings.TrimSpace(cmd), ""))
	f := strings.Fields(strings.SplitN(cmd, "\n", 2)[0])
	// Leading VAR=value assignments aren't the program.
	for len(f) > 1 && strings.Contains(f[0], "=") && !strings.HasPrefix(f[0], "-") {
		f = f[1:]
	}
	if len(f) == 0 {
		return ""
	}
	p := f[0]
	switch p {
	case "git", "go", "npm", "pnpm", "yarn", "cargo", "docker", "kubectl", "gh", "make", "uv", "bun":
		if len(f) > 1 && !strings.HasPrefix(f[1], "-") {
			p += " " + f[1]
		}
	}
	if strings.ContainsAny(cmd, "|;&") {
		p += " …"
	}
	return p
}

var shOps = map[string]bool{"&&": true, "||": true, "|": true, ";": true, ">": true, ">>": true, "<": true, "2>&1": true, "&": true}

func tint(cmd string) string {
	var b strings.Builder
	head := true
	for _, tok := range shellTokens(cmd) {
		switch {
		case strings.TrimSpace(tok) == "":
			b.WriteString(tok)
		case shOps[tok]:
			b.WriteString(paint(cOrange, tok))
			head = true
		case strings.HasPrefix(tok, `"`) || strings.HasPrefix(tok, `'`):
			b.WriteString(paint(cGreen, tok))
			head = false
		case strings.HasPrefix(tok, "$"):
			b.WriteString(paint(cBlue, tok))
			head = false
		case head:
			b.WriteString(paint(cText+bold, tok))
			head = false
		case strings.HasPrefix(tok, "-"):
			b.WriteString(sub(tok))
		default:
			b.WriteString(text(tok))
		}
	}
	return b.String()
}

// shLine is one line of a command as shellLines lays it out: depth 1 is a
// pipe's later stage; verbatim is a heredoc's body, shown as written.
type shLine struct {
	text     string
	depth    int
	verbatim bool
}

// shellLines lays a command out the way a person would write it rather than
// as the one long line an agent sends: each command on its own line, && and
// || leading the line they join, a pipe's stages indented under it. Quotes,
// $(…) and heredoc bodies are never split.
func shellLines(cmd string) []shLine {
	var out []shLine
	var cur strings.Builder
	depth := 0 // of the line being built
	emit := func(next int) {
		if s := strings.TrimSpace(cur.String()); s != "" {
			out = append(out, shLine{text: s, depth: depth})
		}
		cur.Reset()
		depth = next
	}
	quote, parens := byte(0), 0
	heredoc := ""
	for i := 0; i < len(cmd); i++ {
		c := cmd[i]
		switch {
		case quote != 0:
			cur.WriteByte(c)
			if c == '\\' && quote == '"' && i+1 < len(cmd) {
				i++
				cur.WriteByte(cmd[i])
			} else if c == quote {
				quote = 0
			}
		case c == '\\' && i+1 < len(cmd):
			cur.WriteByte(c)
			i++
			cur.WriteByte(cmd[i])
		case c == '\'' || c == '"':
			quote = c
			cur.WriteByte(c)
		case c == '(':
			parens++
			cur.WriteByte(c)
		case c == ')':
			parens = max(0, parens-1)
			cur.WriteByte(c)
		case parens > 0:
			cur.WriteByte(c)
		case c == '<' && strings.HasPrefix(cmd[i:], "<<") && !strings.HasPrefix(cmd[i:], "<<<"):
			// Remember the word that ends the heredoc; its body starts at
			// the next newline.
			rest := strings.TrimLeft(strings.TrimPrefix(cmd[i+2:], "-"), " ")
			end := strings.IndexAny(rest, " \n;|&<>")
			if end < 0 {
				end = len(rest)
			}
			heredoc = strings.Trim(rest[:end], `'"`)
			cur.WriteString("<<")
			i++
		case c == '\n':
			emit(0)
			if heredoc != "" {
				body := cmd[i+1:]
				for n, l := range strings.Split(body, "\n") {
					out = append(out, shLine{text: l, verbatim: true})
					i += len(l) + 1
					if strings.TrimSpace(l) == heredoc || n > 10000 {
						break
					}
				}
				heredoc = ""
			}
		case c == ';':
			emit(0)
		case c == '&' && strings.HasPrefix(cmd[i:], "&&"), c == '|' && strings.HasPrefix(cmd[i:], "||"):
			emit(0)
			cur.WriteString(cmd[i:i+2] + " ")
			i = skipSpaces(cmd, i+1)
		case c == '|' && !(i > 0 && cmd[i-1] == '>'):
			emit(1)
			cur.WriteString("| ")
			i = skipSpaces(cmd, i)
		default:
			cur.WriteByte(c)
		}
	}
	emit(0)
	return out
}

// skipSpaces is i moved past the spaces that follow it.
func skipSpaces(s string, i int) int {
	for i+1 < len(s) && s[i+1] == ' ' {
		i++
	}
	return i
}

// shellTokens splits on spaces, keeping quoted strings and spaces whole.
func shellTokens(s string) []string {
	var out []string
	var cur strings.Builder
	flush := func() {
		if cur.Len() > 0 {
			out = append(out, cur.String())
			cur.Reset()
		}
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '"' || c == '\'':
			flush()
			j := strings.IndexByte(s[i+1:], c)
			if j < 0 {
				out = append(out, s[i:])
				return out
			}
			out = append(out, s[i:i+j+2])
			i += j + 1
		case c == ' ' || c == '\t':
			flush()
			out = append(out, string(c))
		case c == ';' || c == '|' || c == '&':
			flush()
			if i+1 < len(s) && (s[i+1] == c) {
				out = append(out, s[i:i+2])
				i++
			} else {
				out = append(out, string(c))
			}
		default:
			cur.WriteByte(c)
		}
	}
	flush()
	return out
}

// --- summaries ---

var (
	goOK   = regexp.MustCompile(`(?m)^ok\s+\S+`)
	goFail = regexp.MustCompile(`(?m)^(FAIL\s+\S+|--- FAIL)`)
	jsSum  = regexp.MustCompile(`Tests?\s+(?:\x1b\[[0-9;]*m)?(?:(\d+) failed[^|\n]*\|\s*)?(\d+) passed`)
)

func (d *drawer) summary(st *Step) string {
	if st.fan {
		return faint(plural(len(st.Children), "agent"))
	}
	if st.agentRun() != nil {
		if n := len(st.Children); n > 0 {
			return faint(plural(n, "step"))
		}
		return ""
	}
	switch {
	case st.kind() == tool.Shell:
		out := bashOut(st)
		if n := len(goFail.FindAllString(out, -1)); n > 0 {
			s := fmt.Sprintf("%d failed", n)
			var names []string
			for _, m := range goFailName.FindAllStringSubmatch(out, 3) {
				names = append(names, m[1])
			}
			if len(names) > 0 {
				s += ": " + strings.Join(names, ", ")
			}
			return paint(cRed, s)
		}
		if m := jsSum.FindStringSubmatch(out); m != nil {
			if m[1] != "" && m[1] != "0" {
				return paint(cRed, m[1]+" failed") + faint(" · "+m[2]+" passed")
			}
			return faint(m[2] + " passed")
		}
		if n := len(goOK.FindAllString(out, -1)); n > 0 {
			if n == 1 {
				return faint("ok")
			}
			return faint(fmt.Sprintf("%d ok", n))
		}
		if st.Status != OK {
			return ""
		}
		sh := d.shellShape(st.in().Command)
		n := countLines(out)
		switch {
		case sh.kind == "commit":
			res := sh.res
			if m := commitRe.FindStringSubmatch(out); m != nil {
				res = strings.TrimSpace(res + " " + m[1])
			}
			return faint(res)
		case sh.kind == "search" && sh.ctx && n > 0:
			return faint(plural(n, "line"))
		case sh.kind == "search" && n > 0:
			if n == 1 {
				return faint("1 match")
			}
			return faint(fmt.Sprintf("%d matches", n))
		case sh.kind == "search":
			return faint("no matches")
		case sh.res != "":
			return faint(sh.res)
		case n == 0:
			return faint("no output")
		case n > 1:
			return faint(fmt.Sprintf("%d lines", n))
		}
	case st.kind() == tool.Edit || st.kind() == tool.Write:
		o := st.out()
		if o.Created {
			return paint(cGreen, fmt.Sprintf("new · %d lines", countLines(st.in().Content)))
		}
		add, del := 0, 0
		for _, p := range o.Patches {
			for _, l := range p.Lines {
				switch {
				case strings.HasPrefix(l, "+"):
					add++
				case strings.HasPrefix(l, "-"):
					del++
				}
			}
		}
		if add+del > 0 {
			return paint(cGreen, fmt.Sprintf("+%d", add)) + " " + paint(cRed, fmt.Sprintf("−%d", del))
		}
	case st.kind() == tool.Read:
		if f := st.out().Lines; f != nil && f.Count > 0 {
			if f.Count < f.Total {
				return faint(fmt.Sprintf("lines %d–%d", f.Start, f.Start+f.Count-1))
			}
			return faint(fmt.Sprintf("%d lines", f.Total))
		}
	case st.kind() == tool.Search || st.kind() == tool.Glob:
		if st.Status == OK {
			n := countLines(st.Output)
			if strings.HasPrefix(strings.TrimSpace(st.Output), "No ") {
				n = 0
			}
			noun := "results"
			if st.kind() == tool.Glob {
				noun = "files"
			}
			return faint(fmt.Sprintf("%d %s", n, noun))
		}
	case st.kind() == tool.Subagent:
		if n := len(st.Children); n > 0 {
			return faint(plural(n, "step"))
		}
	}
	return toolSummary(st)
}

func bashOut(st *Step) string {
	if o := st.out(); o.Stdout != "" || o.Stderr != "" {
		return o.Stdout + "\n" + o.Stderr
	}
	return st.Output
}

func countLines(s string) int {
	s = strings.TrimRight(s, "\n")
	if strings.TrimSpace(s) == "" {
		return 0
	}
	return strings.Count(s, "\n") + 1
}

// --- bodies ---

func (d *drawer) body(st *Step, indent int) {
	// Code read from files shows highlighted.
	d.lg, d.byPath = nil, false
	d.resetHL()
	defer func() { d.lg, d.byPath, d.spans, d.view, d.nowrap = nil, false, nil, "", false }()
	x := st.in()
	ref := d.ref + ":s:" + st.ID
	d.nowrap = d.o.Open[NoWrapRef(ref)]
	if stopTool(st.Tool) && st.Status != Failed {
		// What it stopped, laid out as a command: the row says the rest.
		if cmd := stoppedCommand(st); cmd != "" {
			d.shellBody(st, cmd, indent)
		}
		return
	}
	if qa := answered(st); st.kind() == tool.Question && qa != nil {
		d.answers(qa, indent)
		return
	}
	// Any subagent's body is what it said back: a shell-run agent's too.
	// The agents a command ran are its body, bar in verbose mode.
	switch {
	case st.run != nil:
		d.output(reply(st), indent, st.Status == Failed)
		return
	case st.toolRun() != nil:
		d.brief(st.toolRun().Prompt, indent)
		d.output(reply(st), indent, st.Status == Failed)
		return
	case st.fan && !d.o.Verbose:
		return
	}
	switch v, _ := d.viewOf(st, ref); {
	case v == ViewHex:
		if cmd := strings.TrimSpace(x.Command); st.kind() == tool.Shell && cmd != "" {
			d.shellBody(st, cmd, indent)
		}
		d.hexBody(st, indent)
		return
	case v == ViewPretty || v == ViewText && d.o.View[ref] == ViewText:
		d.view = v
	}
	switch {
	case st.kind() == tool.Read:
		d.lg = langFor(x.Path)
	case st.kind() == tool.Search:
		d.byPath = true
	case st.kind() == tool.Shell:
		switch sh := d.shellShape(x.Command); sh.kind {
		case "read":
			d.lg = langFor(sh.what)
		case "search":
			d.byPath = true
			if _, p, ok := strings.Cut(sh.what, " in "); ok {
				d.lg = langFor(strings.Split(p, ", ")[0])
			}
		case "":
			d.spans = d.chainSpans(x.Command)
		}
	}
	if d.pictures(st, indent) && strings.TrimSpace(st.Output) == "" {
		return
	}
	switch {
	case st.kind() == tool.Edit || st.kind() == tool.Write:
		if d.diff(st, indent) {
			return
		}
	case st.kind() == tool.Shell:
		if cmd := strings.TrimSpace(st.in().Command); cmd != "" {
			d.shellBody(st, cmd, indent)
			d.marks = echoMarks(cmd)
			defer func() { d.marks = nil }()
		}
		if r := st.out(); r.Stdout != "" || r.Stderr != "" {
			d.output(r.Stdout, indent, st.Status == Failed && r.Stderr == "")
			d.spans, d.view = nil, "" // the views are of stdout
			if strings.TrimSpace(r.Stderr) != "" {
				if strings.TrimSpace(r.Stdout) != "" {
					// A heading, not a fold: every line of it follows.
					d.add("", "", d.spine()+strings.Repeat(" ", indent-1)+paint(cRed, "stderr"), "")
				}
				d.output(strings.TrimLeft(r.Stderr, "\n"), indent, true) // red, whatever it exited with
			}
			return
		}
		if st.Status == OK && strings.TrimSpace(st.Output) == "" {
			d.add("", "", d.spine()+strings.Repeat(" ", indent-1)+faint("no output"), "")
			return
		}
	}
	// Auto mode's card says why; its instructions to Claude are noise.
	if _, ok := classified(st); ok && !d.o.Verbose {
		return
	}
	out := st.Output
	if d.view == ViewPretty && st.kind() == tool.Read {
		out = unnumbered(out)
	}
	d.output(strings.TrimLeft(exitRe.ReplaceAllString(toolErrTag.Replace(out), ""), "\n"), indent, st.Status == Failed)
}

// pictures draws the images a step read or was given back, each a small
// thumbnail that opens its file when clicked, or its chip until the
// thumbnail's made or when it can't be. It says whether there were any.
func (d *drawer) pictures(st *Step, indent int) bool {
	path := ""
	if st.kind() == tool.Read {
		path = d.abs(st.in().Path)
	}
	imgs := st.Images
	if len(imgs) == 0 && thumbable(path) {
		imgs = []*event.ImageData{{Path: path}}
	}
	pad := d.spine() + strings.Repeat(" ", indent-1)
	for i, img := range imgs {
		src := firstNonEmpty(img.Path, path)
		rows, ok := thumbOf(thumbKey{st.ID, i}, img)
		if !ok || cellw.String(rows[0]) > d.cw-indent-2 {
			chip := paint(cBlue, "▣ ") + paint(cText, "image")
			if src != "" {
				chip = imageChip("", src)
			}
			d.add("", "", pad+chip, "")
			continue
		}
		for _, r := range rows {
			d.add("", "", pad+d.fileLink(src, r), "")
		}
	}
	return len(imgs) > 0
}

// shellBody is an opened shell step's command: on one line when it fits,
// else laid out a command a line; a heredoc's body shows its first lines.
// A chain seen running says, beside each command, how long it ran, and
// the one running now reads brighter.
func (d *drawer) shellBody(st *Step, cmd string, indent int) {
	if v := d.o.Scripts[st.ID]; v != nil && st.ID != "" {
		d.scriptDoc(st, v, cmd, indent)
		return
	}
	pad := d.spine() + strings.Repeat(" ", indent-1)
	room := d.cw - indent - 4
	raw := shellLines(cmd)
	// A script (a loop, an if, a group) reads as one: its statements aren't
	// a chain's commands, with a time and a running marker each.
	script := compound(raw)
	var marks []string
	if !script {
		marks = d.partMarks(st, len(segments(cmd)))
	}
	if marks != nil {
		room -= 10
	}
	if marks == nil && !strings.Contains(cmd, "\n") && cellw.String(cmd) <= room {
		d.add("", bgWell, pad+faint("$ ")+quietTint(expandTabs(cmd)), "")
		return
	}
	body := 0
	// A pipe carries on its command's line while that still fits; each
	// command of a chain keeps a line of its own, the && or || that joins it
	// out in the margin so the commands line up.
	var lines []shLine
	gutter := 2
	if script {
		lines, raw = scriptLines(raw), nil
	}
	for _, l := range raw {
		if n := len(lines); n > 0 && strings.HasPrefix(l.text, "| ") && !lines[n-1].verbatim && cellw.String(lines[n-1].text)+1+cellw.String(l.text)+lines[n-1].depth*2 <= room {
			lines[n-1].text += " " + l.text
			continue
		}
		if !l.verbatim && (strings.HasPrefix(l.text, "&& ") || strings.HasPrefix(l.text, "|| ")) {
			gutter = 3
		}
		lines = append(lines, subshellLines(l)...)
	}
	// A heredoc's lines are numbered as what reads it counts them; the
	// word that ends it isn't one. A script fed that way which failed
	// shows the line it stopped at, and what went wrong there.
	no := make([]int, len(lines))
	docs, most := 0, 0
	for i, l := range lines {
		if !l.verbatim {
			continue
		}
		body++
		if i == 0 || !lines[i-1].verbatim {
			docs++
		}
		if i+1 < len(lines) && lines[i+1].verbatim {
			no[i] = 1
			if i > 0 && lines[i-1].verbatim {
				no[i] = no[i-1] + 1
			}
			most = max(most, no[i])
		}
	}
	fault, says := 0, ""
	if docs == 1 && (st.Status == Failed || d.testsFailed(st)) {
		fault, says = stdinFault(bashOut(st))
	}
	numW := len(strconv.Itoa(most))
	// What a long heredoc keeps in view: its start, its end word, and the
	// lines round where it failed.
	shown := func(i int) bool {
		n := no[i]
		return d.o.Verbose || body <= 8 || n <= 6 || fault > 0 && n >= fault-1 && n <= fault+1 // n is 0 for the end word
	}
	blank := strings.Repeat(" ", gutter)
	var lg *lang
	var hs hlState
	part := -1
	for i, l := range lines {
		mark := ""
		if !l.verbatim && l.depth == 0 {
			if part++; part < len(marks) {
				mark = marks[part]
			}
		}
		now := !script && part == st.at && st.parts[part] != nil && st.parts[part].end.IsZero() && d.s.live(st)
		lead := faint("$") + blank[1:] // the command, not each line of a heredoc
		if i > 0 {
			lead = blank
		}
		if op := l.text[:min(3, len(l.text))]; !script && !l.verbatim && (op == "&& " || op == "|| ") {
			lead, l.text = paint(cOrange, op[:2])+" ", l.text[3:]
		}
		if !l.verbatim && strings.Contains(l.text, "<<") {
			lg, hs = heredocLang(l.text), hlState{}
		}
		if l.verbatim && !shown(i) {
			if i > 0 && shown(i - 1) {
				hid := 0
				for j := i; j < len(lines) && lines[j].verbatim && !shown(j); j++ {
					hid++
				}
				d.add("", bgWell, pad+lead+blanks(numW+2)+folded(hid), "")
			}
			continue
		}
		num := ""
		if l.verbatim {
			num = blanks(numW + 2)
			if no[i] > 0 {
				num = faint(fmt.Sprintf("%*d  ", numW, no[i]))
			}
			if no[i] > 0 && no[i] == fault {
				num = paint(cRed+bold, fmt.Sprintf("%*d", numW, no[i])) + paint(cRed, "▸ ")
			}
		}
		hang := strings.Repeat(" ", l.depth*2)
		colored := quietTint(expandTabs(l.text))
		if script && !l.verbatim {
			var st hlState // each statement stands alone: a quote can't open across lines
			colored = highlight(langSh, &st, expandTabs(l.text), cSub, nil)
		}
		if now && !l.verbatim {
			colored = paint(cText+bold, expandTabs(l.text))
			if lead == blank {
				lead = paint(cOrange, "▸") + blank[1:]
			}
		}
		if l.verbatim {
			colored = highlight(lg, &hs, expandTabs(l.text), cOut, nil)
			if no[i] == 0 {
				colored = faint(l.text) // the word that ends it
			}
		}
		for j, r := range wrap(colored, room-len(hang)-cellw.String(num)) {
			if j > 0 {
				lead, r, mark = blank, "  "+r, "" // a wrapped line hangs under its own start
				if num != "" {
					num = blanks(numW + 2)
				}
			}
			d.add("", bgWell, pad+lead+num+hang+r, mark)
			if j > 0 {
				d.wrapped()
			}
		}
		if l.verbatim && no[i] > 0 && no[i] == fault && says != "" {
			for _, r := range wrap(paint(cRed, "✗ "+says), room-numW-2) {
				d.add("", bgWell, pad+blank+blanks(numW+2)+r, "")
			}
		}
	}
}

// subshellLines lays out a command that's a subshell of several, "(a; b)",
// a command a line inside its brackets. The lines after the first are
// deeper, so the whole still counts as one command of its chain.
func subshellLines(l shLine) []shLine {
	op, body := "", l.text
	if o := body[:min(3, len(body))]; o == "&& " || o == "|| " {
		op, body = o, body[3:]
	}
	if l.verbatim || l.depth > 0 || !strings.HasPrefix(body, "(") || !strings.HasSuffix(body, ")") {
		return []shLine{l}
	}
	inner := shellLines(body[1 : len(body)-1])
	if len(inner) < 2 || closesEarly(body) {
		return []shLine{l}
	}
	out := make([]shLine, 0, len(inner))
	for i, in := range inner {
		t := in.text // under the first, past its "( "
		if i == 0 {
			t = op + "( " + in.text
		}
		if i == len(inner)-1 {
			t += ")"
		}
		out = append(out, shLine{text: t, depth: 1 + in.depth, verbatim: in.verbatim})
	}
	out[0].depth = 0
	return out
}

// closesEarly is whether the bracket that opens s closes before its end:
// "(a) && (b)" is two subshells, not one.
func closesEarly(s string) bool {
	depth, quote := 0, byte(0)
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case quote != 0:
			if c == '\\' && quote == '"' {
				i++
			} else if c == quote {
				quote = 0
			}
		case c == '\'' || c == '"':
			quote = c
		case c == '(':
			depth++
		case c == ')':
			if depth--; depth == 0 && i < len(s)-1 {
				return true
			}
		}
	}
	return false
}

// errRe finds the line of a failure's output that says what went wrong.
var errRe = regexp.MustCompile(`(?i)(error|fatal|panic|exception|traceback|failed|\bfail\b|not found|no such|denied|cannot|can't|undefined|unexpected|invalid|refused|timed out)`)

var (
	// tallyRe is a test runner's verdict line, which says only that it failed.
	tallyRe = regexp.MustCompile(`^(FAIL|ok|PASS)(\s+\S+(\s+[\d.]+s|\s+\[[^\]]+\])?)?$|^exit status \d+$`)
	// fileLineRe is a message placed at a file and line: path.go:12: or :12:3:.
	fileLineRe = regexp.MustCompile(`^\S+\.\w+:\d+(:\d+)?: \S`)
)

// errorLine is a failure in brief: the line of its output that says what
// went wrong (the last one that reads like an error, else the last line),
// in red on the failure's surface, with the way to see the rest.
func (d *drawer) errorLine(st *Step, indent int, ref string) {
	text := st.Output
	if st.kind() == tool.Shell {
		if r := st.out(); r.Stdout+r.Stderr != "" {
			text = r.Stdout + "\n" + r.Stderr
		}
	}
	// The harness's refusal says why in its own words; its tags don't.
	if m := blockedRe.FindStringSubmatch(st.Output); m != nil && st.Status == Failed {
		text = m[1]
	}
	text = toolErrTag.Replace(text)
	var last, hit, at string
	for _, l := range strings.Split(stripANSI(collapseCR(text)), "\n") {
		l = strings.TrimSpace(expandTabs(l))
		if l == "" || exitRe.MatchString(l) && errRe.FindString(l) == "" || tallyRe.MatchString(l) {
			continue
		}
		last = l
		if errRe.MatchString(l) {
			hit = l
		}
		// The first file:line: message is where it went wrong: a compiler's
		// first error, a failing test's own words.
		if at == "" && fileLineRe.MatchString(l) {
			at = l
		}
	}
	if at != "" {
		hit = at
	}
	if hit == "" {
		hit = last
	}
	if hit == "" {
		return
	}
	// One quiet line: the error itself, cut to fit; the rest is a key away.
	pad := d.spine() + strings.Repeat(" ", indent-3)
	right := ""
	if d.o.Selected == ref {
		right = dim("enter shows all")
	}
	w := d.cw - indent - 20
	d.add(ref, "", pad+faint("▸ ")+dim(truncateCells(hit, max(20, w))), right)
}

// output draws text in a well: head and tail when it's long, all of it in
// verbose mode, tinted red when it's a failure.
// brief draws what a relayed agent was asked: a few lines, the rest behind
// the same ctrl+o as any long output.
func (d *drawer) brief(prompt string, indent int) {
	lines := strings.Split(strings.TrimSpace(prompt), "\n")
	show := len(lines)
	if !d.o.Verbose && show > briefRows {
		show = briefRows
	}
	pad := d.spine() + strings.Repeat(" ", indent-1)
	for _, l := range lines[:show] {
		d.add("", bgWell, pad+faint("▏")+" "+dim(expandTabs(l)), "")
	}
	if show < len(lines) {
		d.add("", bgWell, pad+dim(fmt.Sprintf("… %d more lines ", len(lines)-show))+faint("·")+" "+paint(cOrange+bold, "ctrl+o")+dim(" shows all"), "")
	}
}

// briefRows is how much of a relayed agent's task shows until it's opened.
const briefRows = 4

func (d *drawer) output(s string, indent int, failed bool) {
	s = unscreen(collapseCR(strings.TrimRight(s, "\n")))
	s = d.notices(s, indent)
	if strings.TrimSpace(s) == "" {
		return
	}
	// JSON a tool printed (an API's answer, a --json flag, an MCP result)
	// is laid out and coloured, however it came.
	// Switched to pretty, it's laid out however it came; switched to text,
	// JSON stays as it came, coloured.
	isJSON, hl := false, langJSON
	if p, lg, ok := prettyText(s); ok && d.view == ViewPretty {
		s, isJSON, hl, d.hs = p, true, lg, hlState{}
		d.lg, d.byPath, d.spans = nil, false, nil
	} else if !failed && d.lg == nil && !d.byPath {
		if p, ok := prettyJSON(s); ok {
			isJSON, d.hs = true, hlState{}
			if d.view != ViewText {
				s = p
			}
		}
	}
	lines := strings.Split(s, "\n")
	b, edge := bgWell, faint("▏")
	pad := d.spine() + strings.Repeat(" ", indent-1)
	defer d.widen(lines, indent+2)()
	w := d.cw - indent - 2
	most := 6
	if d.nowrap {
		most = 1
	}
	put := func(b, lead, body string) {
		d.addRows(b, pad+edge, lead, body, w-cellw.String(lead), most)
	}
	// What a tool printed is quieter than anything Claude says, and plain
	// text: a heading or a list in a file never passes for Claude's own.
	// A failure's error lines stay red.
	// A chain's parts each have their own language, in the order it ran them.
	// A part of known length covers that many lines; one of unknown length
	// runs until a line that looks like the start of the next and not like
	// its own.
	spanOf := make([]int, 0, len(lines))
	for j, k := 0, 0; len(d.spans) > 0 && len(spanOf) < len(lines); k++ {
		sp, l := d.spans[j], cleanOutput(lines[len(spanOf)])
		if sp.n > 0 && k >= sp.n || sp.n == 0 && j+1 < len(d.spans) && d.spans[j+1].starts(l) && !sp.starts(l) {
			if j++; j == len(d.spans) {
				break
			}
			k = 0
		}
		spanOf = append(spanOf, j)
	}
	// A diff is drawn as an edit is: each file's header, and its lines
	// numbered and highlighted in the file's language.
	var ud *udiff
	if !failed && len(spanOf) > 0 {
		ud = parseDiff(lines, func(i int) bool { return i < len(spanOf) && d.spans[spanOf[i]].diff })
	}
	// A diff the command didn't say it prints (git -C, gh pr diff, a pipe
	// rush can't read) is found in the output; the lines around it stay as
	// they are.
	found := false
	if ud == nil && !failed && !isJSON && hasDiff(lines) {
		ud = parseDiff(lines, func(int) bool { return true })
		found = ud != nil
	}
	diffLine := func(i int, l string) bool {
		if found {
			return ud.line(d, pad+edge, w, d.lg, lines, i)
		}
		if ud == nil || i >= len(spanOf) || !d.spans[spanOf[i]].diff {
			return false
		}
		if ud.line(d, pad+edge, w, d.spans[spanOf[i]].lg, lines, i) {
			return true
		}
		// A commit's header and message, as git log shows them.
		put(b, "", d.gitLine("log", l))
		return true
	}
	// code is how long line i's prefix is, the path in it and the language
	// its code is in; a nil language when it isn't code.
	code := func(i int, l string) (int, string, *lang) {
		lg, byPath := d.lg, d.byPath
		if i < len(spanOf) {
			lg, byPath = d.spans[spanOf[i]].lg, d.spans[spanOf[i]].byPath
		}
		if failed || lg == nil && !byPath || i < len(spanOf) && d.spans[spanOf[i]].diff || found && ud.ls[i].kind != 0 {
			return 0, "", nil
		}
		n, path := codePrefix(l)
		if byPath && path != "" {
			lg = langFor(path)
		}
		return n, path, lg
	}
	// Code lines start in one column after their prefixes, less the
	// indentation they all share: a chain's parts each line up their own,
	// and lines with no prefix aren't pushed along to meet the others.
	group := func(i, n int) int {
		g := 0
		if i < len(spanOf) {
			g = spanOf[i] + 1
		}
		if n > 0 {
			return g*2 + 1
		}
		return g * 2
	}
	// A hit's path goes above it once per file, as rg --heading has it, so
	// no gutter is as wide as the longest path; its line numbers line up
	// with its own file's only.
	prefixW, shared := make([]int, 2*len(d.spans)+2), make([]int, 2*len(d.spans)+2)
	numW := map[string]int{}
	for g := range shared {
		shared[g] = -1
	}
	// Hits with a line number share indentation only with those near them
	// in the same file: hits from different files, or far apart, have
	// nothing to line up, and each is brought left.
	hunk, hunkInd := make([]int, len(lines)), map[int]int{}
	prevPath, prevNo, h := "", -2, 0
	for i, l := range lines {
		hunk[i] = -1
		l = expandTabs(cleanOutput(l))
		n, path, lg := code(i, l)
		if lg == nil || strings.TrimSpace(l[n:]) == "" {
			continue
		}
		g := group(i, n)
		if path != "" {
			numW[path] = max(numW[path], len(strconv.Itoa(lineNo(l[:n]))))
		} else {
			prefixW[g] = max(prefixW[g], cellw.String(l[:n]))
		}
		ind := len(l[n:]) - len(strings.TrimLeft(l[n:], " "))
		if no := lineNo(l[:n]); n > 0 && no > 0 {
			if path != prevPath || no <= prevNo || no > prevNo+20 {
				h++
				hunkInd[h] = ind
			}
			prevPath, prevNo, hunk[i] = path, no, h
			hunkInd[h] = min(hunkInd[h], ind)
			continue
		}
		if shared[g] < 0 || ind < shared[g] {
			shared[g] = ind
		}
	}
	last := -1
	head := "" // the path last put above its hits
	var hex map[int]string
	if !failed && !isJSON {
		hex = hexRows(lines)
	}
	emit := func(i int, l string) {
		if r, ok := hex[i]; ok {
			put(b, "", r) // od's bytes as a hex viewer draws them
			return
		}
		// A tool's own escape codes (colours, cursor moves, titles) would
		// reach the terminal or throw widths off; rush does the colour.
		l = expandTabs(cleanOutput(l))
		if d.marks[strings.TrimSpace(l)] {
			put(b, "", markHeading(strings.TrimSpace(l), w))
			return
		}
		if i < len(spanOf) && !failed {
			if k := spanOf[i]; k != last {
				d.resetHL()
				last = k
			}
		}
		if diffLine(i, l) {
			return
		}
		if n, path, lg := code(i, l); lg != nil {
			d.carry(path, l[:n])
			g := group(i, n)
			pre := l[:n] + strings.Repeat(" ", max(0, prefixW[g]-cellw.String(l[:n])))
			body := l[n:]
			sh := shared[g]
			if hunk[i] >= 0 {
				sh = hunkInd[hunk[i]]
			}
			if sh > 0 && len(body)-len(strings.TrimLeft(body, " ")) >= sh {
				body = body[sh:]
			}
			if lg == langMD && d.hs.fence == nil && !mdTable(body) {
				body = squeeze(body) // prose isn't lined up in columns: a gap is a gap
			}
			if path != "" {
				if path != head {
					put(b, "", paint(cSub, path))
					head = path
				}
				pre = fmt.Sprintf("%*d  ", numW[path], lineNo(l[:n]))
			}
			put(b, faint(pre), highlight(lg, &d.hs, body, cOut, nil))
			return
		}
		if i < len(spanOf) && !failed && d.spans[spanOf[i]].git != "" {
			put(b, "", d.gitLine(d.spans[spanOf[i]].git, l))
			return
		}
		if isJSON {
			put(b, "", highlight(hl, &d.hs, l, cOut, nil))
			return
		}
		put(b, "", paint(cOut, l))
	}
	more := func(n int) {
		d.resetHL() // what follows the gap doesn't go on from what came before it
		head = ""
		put(b, "", folded(n))
	}
	if !d.o.Verbose && ud != nil && ud.whole {
		// A diff reads from the top, as far as an edit shows.
		rows := 0
		for i, l := range lines {
			if rows >= 40 {
				more(len(lines) - i)
				return
			}
			if ud.drawn(i) {
				rows++
			}
			emit(i, l)
		}
		return
	}
	if !d.o.Verbose && len(lines) > 8 {
		// The top and the end, where results and errors land; a failure
		// also keeps the error lines from the middle.
		for i, l := range lines[:3] {
			emit(i, l)
		}
		mid := lines[3 : len(lines)-5]
		// A part's heading stays between its folds only when there are a
		// couple: many parts would read as a column of folds and nothing else.
		heads := 0
		for _, l := range mid {
			if d.marks[strings.TrimSpace(l)] {
				heads++
			}
		}
		cut := 0
		kept := 0
		for i, l := range mid {
			if failed && kept < 3 && errRe.MatchString(l) || heads <= 2 && d.marks[strings.TrimSpace(l)] {
				if cut > 0 {
					more(cut)
					cut = 0
				}
				emit(3+i, l)
				kept++
				continue
			}
			cut++
		}
		if cut > 0 {
			more(cut)
		}
		for i, l := range lines[len(lines)-5:] {
			emit(len(lines)-5+i, l)
		}
		return
	}
	for i, l := range lines {
		if i >= 2000 {
			put(b, "", faint(fmt.Sprintf("… %d more lines", len(lines)-i)))
			break
		}
		emit(i, l)
	}
}

func (d *drawer) resetHL() {
	d.hs, d.hsPath, d.hsN = hlState{}, "", 0
}

// carry keeps the highlighter's state from the line before only when this
// line goes on from it in the same file: grep's matches are fragments, and
// a string or comment one leaves open mustn't colour the next.
func (d *drawer) carry(path, prefix string) {
	n := lineNo(prefix)
	if path != d.hsPath || n != 0 && d.hsN != 0 && n != d.hsN+1 {
		d.hs = hlState{}
	}
	d.hsPath, d.hsN = path, n
}

// prettyJSON is s indented two spaces a level when it's one JSON object or
// array, as is when it's JSON lines; ok is false for anything else.
func prettyJSON(s string) (string, bool) {
	t := strings.TrimSpace(s)
	if len(t) < 2 || len(t) > 4<<20 || t[0] != '{' && t[0] != '[' {
		return "", false
	}
	if jsonx.Valid([]byte(t)) {
		out, err := jsonx.Indent([]byte(t))
		if err != nil {
			return "", false
		}
		return string(out), true
	}
	for _, l := range strings.Split(t, "\n") {
		if l = strings.TrimSpace(l); l != "" && (l[0] != '{' && l[0] != '[' || !jsonx.Valid([]byte(l))) {
			return "", false
		}
	}
	return t, true
}

func (d *drawer) diff(st *Step, indent int) bool {
	o := st.out()
	lg := langFor(st.in().Path)
	pad := d.spine() + strings.Repeat(" ", indent-1)
	w := d.cw - indent - 9
	if o.Created {
		// A new file is a diff where every line is added.
		var hs hlState
		lines := strings.Split(strings.TrimRight(st.in().Content, "\n"), "\n")
		for i, l := range lines {
			if i >= 30 && !d.o.Verbose {
				d.add("", bgWell, pad+faint(fmt.Sprintf("  … %d more lines · ctrl+o shows the rest", len(lines)-i)), "")
				break
			}
			d.diffLine(pad, w, lg, &hs, '+', i+1, l, "", false)
		}
		return true
	}
	patches := o.Patches
	if len(patches) == 0 {
		return false
	}
	shown := 0
	for pi, p := range patches {
		if pi > 0 {
			d.add("", bgWell, pad+faint("  ⋯"), "")
		}
		// The old and the new side each carry their own strings and
		// comments from line to line.
		var oldSt, newSt hlState
		oldN, newN := p.OldStart, p.NewStart
		ls := p.Lines
		for i := 0; i < len(ls); i++ {
			// A run of removed lines and the added lines after it pair up
			// in order, so each pair can show the words that changed.
			if l := ls[i]; l != "" && l[0] == '-' {
				j := i
				for j < len(ls) && ls[j] != "" && ls[j][0] == '-' {
					j++
				}
				k := j
				for k < len(ls) && ls[k] != "" && ls[k][0] == '+' {
					k++
				}
				dels, adds := ls[i:j], ls[j:k]
				for n, l := range dels {
					if shown >= 30 && !d.o.Verbose {
						d.add("", bgWell, pad+faint("  … ctrl+o shows the rest"), "")
						return true
					}
					shown++
					var pair string
					if n < len(adds) {
						pair = adds[n][1:]
					}
					d.diffLine(pad, w, lg, &oldSt, '-', oldN, l[1:], pair, n < len(adds))
					oldN++
				}
				for n, l := range adds {
					if shown >= 30 && !d.o.Verbose {
						d.add("", bgWell, pad+faint("  … ctrl+o shows the rest"), "")
						return true
					}
					shown++
					var pair string
					if n < len(dels) {
						pair = dels[n][1:]
					}
					d.diffLine(pad, w, lg, &newSt, '+', newN, l[1:], pair, n < len(dels))
					newN++
				}
				i = k - 1
				continue
			}
			if shown >= 30 && !d.o.Verbose {
				d.add("", bgWell, pad+faint("  … ctrl+o shows the rest"), "")
				return true
			}
			shown++
			l := ls[i]
			if l == "" {
				l = " "
			}
			if l[0] == '+' {
				d.diffLine(pad, w, lg, &newSt, '+', newN, l[1:], "", false)
				newN++
				continue
			}
			for i, r := range d.codeRows(highlight(lg, &newSt, expandTabs(l[1:]), cSub, nil), w, 6) {
				if i == 0 {
					d.add("", bgWell, pad+faint(fmt.Sprintf("%5d ", newN))+"  "+r, "")
				} else {
					d.add("", bgWell, pad+blanks(8)+r, "")
					d.wrapped()
				}
			}
			oldSt = newSt // a line both sides share leaves them alike
			oldN++
			newN++
		}
	}
	return true
}

// diffLine is one added (+) or removed (-) line at number n, its code
// highlighted on the line's colour and, when it pairs with a line on the
// other side, the words that changed a step brighter.
func (d *drawer) diffLine(pad string, w int, lg *lang, st *hlState, sign byte, n int, code, pair string, paired bool) {
	body, other := expandTabs(code), expandTabs(pair)
	if showSpace {
		body, other = stripANSI(code), stripANSI(pair) // tabs drawn as →
	}
	row, hi, mark := bgAdd, bgAddHi, plusSign()
	if sign == '-' {
		row, hi, mark = bgDel, bgDelHi, minusSign()
	}
	var em *emph
	if paired {
		if from, to, ok := changed(body, other); ok {
			em = &emph{from: from, to: to, on: hi, off: row}
		}
	}
	for i, r := range d.codeRows(paintCode(lg, st, body, cText, em, showSpace), w, 6) {
		if i == 0 {
			d.add("", row, pad+faint(fmt.Sprintf("%5d ", n))+mark+" "+r, "")
		} else {
			d.add("", row, pad+blanks(8)+r, "")
			d.wrapped()
		}
	}
}

// widen lets a block whose lines don't fit where rows usually stop use the
// pane's whole width, lead cells taken before each line; call what it
// returns when the block is drawn.
func (d *drawer) widen(lines []string, lead int) func() {
	cw := d.cw
	if cw >= d.o.Width {
		return func() {}
	}
	for _, l := range lines {
		// No wider than its bytes, a tab four: most lines fit on that alone.
		if len(l)+3*strings.Count(l, "\t")+lead <= cw {
			continue
		}
		if cellw.String(expandTabs(l))+lead > cw {
			d.cw = d.o.Width
			break
		}
	}
	return func() { d.cw = cw }
}

// addRows adds body after pad and lead, wrapped to w cells with the rows it
// carries on to lined up under its start, most rows at most.
func (d *drawer) addRows(b, pad, lead, body string, w, most int) {
	for i, r := range d.codeRows(body, w, most) {
		if i == 0 {
			d.add("", b, pad+lead+r, "")
			continue
		}
		d.add("", b, pad+blanks(cellw.String(lead))+r, "")
		d.wrapped()
	}
}

// codeRows wraps a highlighted line to rows w cells wide, at a space or
// after a hyphen where it can, keeping its colours across the breaks. Past
// most rows the rest is cut, unless verbose or most is 0.
func (d *drawer) codeRows(s string, w, most int) []string {
	w = max(w, 4)
	if cellw.String(s) <= w {
		return []string{s}
	}
	rows := strings.Split(ansi.Wrap(s, w, "-"), "\n")
	for len(rows) > 1 && strings.TrimSpace(ansi.Strip(rows[len(rows)-1])) == "" {
		rows[len(rows)-2] += rows[len(rows)-1] // a row of style codes alone
		rows = rows[:len(rows)-1]
	}
	if (!d.o.Verbose || d.nowrap) && most > 0 && len(rows) > most {
		rows = append(rows[:most-1], cellw.Truncate(rows[most-1], w-1, "")+"›")
	}
	return CarryStyle(rows)
}

// collapseCR keeps only the last state of lines redrawn with carriage
// returns, so progress bars show where they ended.
func collapseCR(s string) string {
	if !strings.Contains(s, "\r") {
		return s
	}
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		if j := strings.LastIndex(strings.TrimRight(l, "\r"), "\r"); j >= 0 {
			lines[i] = l[j+1:]
		}
	}
	return strings.Join(lines, "\n")
}

// diffText is a line of a diff highlighted in lg over colour c, cut to w
// cells, its spaces and tabs marked when that's on.
func diffText(lg *lang, st *hlState, s, c string, w int) string {
	if !showSpace {
		return highlight(lg, st, truncateCells(expandTabs(s), w), c, nil)
	}
	return cellw.Truncate(paintCode(lg, st, stripANSI(s), c, nil, true), max(w, 4), "›")
}

// mdTable is whether a Markdown line, past a read's line number, is a
// table's row: its columns are lined up on purpose.
func mdTable(l string) bool {
	return strings.HasPrefix(strings.TrimSpace(strings.TrimLeft(strings.TrimSpace(l), "0123456789")), "|")
}

// squeeze keeps a line's indentation and makes each wider gap after it two
// spaces.
func squeeze(s string) string {
	t := strings.TrimLeft(s, " ")
	return s[:len(s)-len(t)] + wideGap.ReplaceAllString(t, "  ")
}

var wideGap = regexp.MustCompile(`   +`)

func expandTabs(s string) string { return strings.ReplaceAll(stripANSI(s), "\t", "    ") }

func truncateCells(s string, w int) string {
	if w < 4 {
		w = 4
	}
	r := []rune(s)
	if len(r) > w {
		return string(r[:w-1]) + "›"
	}
	return s
}

func plural(n int, noun string) string {
	if n == 1 {
		return "1 " + noun
	}
	return fmt.Sprintf("%d %ss", n, noun)
}

// figure draws what Claude showed with rush's show tool: the drawing as it
// was sent, in a frame of its own as wide as the pane allows, never wrapped
// or folded. The top edge carries its title and is the row you pick to copy
// it. It reports false while there is no drawing yet to show.
func (d *drawer) figure(st *Step, ref string, indent int) bool {
	rows := Drawing(st)
	if rows == nil {
		return false
	}
	var in agtools.ShowInput
	_ = jsonx.Unmarshal(st.Input, &in)
	title := firstNonEmpty(oneLine(in.Title), "drawing")
	pad := d.spine() + blanks(indent-1)
	wide := 0
	for _, r := range rows {
		wide = max(wide, cellw.String(r))
	}
	room := max(8, d.o.Width-cellw.String(pad)-4) // "│ " … " │"
	if n := len(d.lines); n > 0 && strings.TrimSpace(stripANSI(d.lines[n-1].Text)) != strings.TrimSpace(stripANSI(d.spine())) {
		d.blank()
	}
	inner := min(max(wide, cellw.String(title)+6), room)
	edge := func(l, r, label string) string {
		head := faint(l + "─")
		if label != "" {
			head += " " + label + " "
		}
		fill := inner + 2 - cellw.String(head) + 1
		if fill < 1 {
			head = cellw.Truncate(head, inner+1, "…")
			fill = inner + 3 - cellw.String(head)
		}
		return pad + head + faint(strings.Repeat("─", max(0, fill))+r)
	}
	d.addWide(ref, edge("╭", "╮", glyphColor("◇")+" "+text(title)))
	for _, r := range rows {
		if cellw.String(r) > inner {
			r = cellw.Truncate(r, inner-1, "") + faint("›")
		}
		d.addWide("", pad+faint("│")+" "+paint(cWhite, r)+blanks(inner-cellw.String(r))+" "+faint("│"))
	}
	foot := ""
	if wide > inner {
		foot = dim(fmt.Sprintf(KeyWord("%d more columns · pick it, alt+c copies it whole"), wide-inner))
	}
	d.addWide("", edge("╰", "╯", foot))
	d.blank()
	return true
}

// Drawing is the drawing a show step carries, one string a row, tabs as
// spaces and blank rows at either end dropped; nil if it has none yet.
func Drawing(st *Step) []string {
	if st == nil || st.Tool != agtools.Show {
		return nil
	}
	var in agtools.ShowInput
	if jsonx.Unmarshal(st.Input, &in) != nil {
		return nil
	}
	rows := strings.Split(expandTabs(collapseCR(in.Drawing)), "\n")
	for len(rows) > 0 && strings.TrimSpace(rows[0]) == "" {
		rows = rows[1:]
	}
	for len(rows) > 0 && strings.TrimSpace(rows[len(rows)-1]) == "" {
		rows = rows[:len(rows)-1]
	}
	for i, r := range rows {
		rows[i] = strings.TrimRight(r, " ")
	}
	if len(rows) == 0 {
		return nil
	}
	return rows
}

// addWide is add for a row that may use the pane's whole width rather than
// stopping where numbers line up.
func (d *drawer) addWide(ref, left string) {
	if ref != "" && ref == d.o.Selected {
		left = cursor() + strings.TrimPrefix(left, d.spine())
	}
	d.lines = append(d.lines, Line{Text: row("", left, "", d.o.Width, d.o.Width), Ref: ref})
}

// noUnitMemo draws every unit anew, for tests to compare against.
var noUnitMemo bool

// messageLinks gives paste and image chips a route into the full-message viewer.
func messageLinks(s, ref string) string {
	link := func(chip string) string { return "\x1b]8;;rush:message/" + ref + "\x1b\\" + chip + "\x1b]8;;\x1b\\" }
	s = PasteChipRe.ReplaceAllStringFunc(s, link)
	return messageImageRe.ReplaceAllStringFunc(s, func(chip string) string {
		id := messageImageRe.FindStringSubmatch(chip)[1]
		return "\x1b]8;;rush:message/" + ref + "/image/" + id + "\x1b\\" + chip + "\x1b]8;;\x1b\\"
	})
}

var messageImageRe = regexp.MustCompile(`\[Image #(\d+)[^\]]*\]`)
