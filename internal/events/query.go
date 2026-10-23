package events

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Reading the log back, for rush events and rush why: newest first, from
// the end of the file, stopping as soon as there's enough. A full log is
// 16MB; a question about the last few minutes reads the last few KB of it.

// Line is a line of the log: what it's filtered and linked by, read
// without decoding the rest, and the line itself.
type Line struct {
	At                             time.Time
	Kind, Proc, Session, Ev, Cause string
	From, Xid, SpawnedBy           string
	Failed                         bool // its kind says something failed, or it has an err
	Raw                            []byte
}

// Filter is what lines a query wants. Its zero value wants every one.
type Filter struct {
	Sessions []string  // any of these
	Kinds    []string  // any of these; one ending in * is any kind it starts
	Proc     string    // which of rush's processes logged it
	Since    time.Time // none before
	Grep     string    // the line has this, in any case
	Limit    int       // at most this many; 0 is no limit
}

func (f *Filter) match(l *Line, grep []byte) bool {
	if !l.At.IsZero() && l.At.Before(f.Since) {
		return false
	}
	if len(f.Sessions) > 0 && !slices.Contains(f.Sessions, l.Session) {
		return false
	}
	if f.Proc != "" && l.Proc != f.Proc {
		return false
	}
	if len(f.Kinds) > 0 && !slices.ContainsFunc(f.Kinds, func(k string) bool {
		if p, ok := strings.CutSuffix(k, "*"); ok {
			return strings.HasPrefix(l.Kind, p)
		}
		return k == l.Kind
	}) {
		return false
	}
	return grep == nil || bytes.Contains(bytes.ToLower(l.Raw), grep)
}

// Query is the lines of rush's log that f wants, newest first, and whether
// more matched past f.Limit.
func Query(f Filter) (lines []Line, more bool, err error) {
	var grep []byte
	if f.Grep != "" {
		grep = bytes.ToLower([]byte(f.Grep))
	}
	err = eachBack(f.Since, func(l Line) bool {
		if !f.match(&l, grep) {
			return true
		}
		if f.Limit > 0 && len(lines) == f.Limit {
			more = true
			return false
		}
		lines = append(lines, l)
		return true
	})
	return lines, more, err
}

// Why is what happened to session since: its own lines, those of the
// sessions linked to it (linked, and any the log shows sending to it,
// started by it or starting it), rush's own failures, and the lines that
// led to any of those. Oldest first.
func Why(session string, linked []string, since time.Time) ([]Line, error) {
	var all []Line
	if err := eachBack(since, func(l Line) bool {
		if !l.At.Before(since) {
			all = append(all, l)
		}
		return true
	}); err != nil {
		return nil, err
	}
	slices.Reverse(all)
	near := map[string]bool{session: true}
	for _, s := range linked {
		near[s] = true
	}
	for _, l := range all {
		switch {
		case l.Session == session:
			near[l.From], near[l.SpawnedBy] = true, true
		case l.From == session || l.SpawnedBy == session:
			near[l.Session] = true
		}
	}
	delete(near, "")
	keep := make([]bool, len(all))
	at := evs(all)
	for i, l := range all {
		if near[l.Session] || l.Session == "" && l.Failed {
			keep[i] = true
			// And what led to it, back to the start of the window.
			for c := l.Cause; c != ""; {
				j, ok := at[c]
				if !ok || keep[j] {
					break
				}
				keep[j] = true
				c = all[j].Cause
			}
		}
	}
	var out []Line
	for i, l := range all {
		if keep[i] {
			out = append(out, l)
		}
	}
	return out, nil
}

// Chain is the chain of causes line ev is in: the line nothing led to that
// it comes from, and everything that came of that. Oldest first.
func Chain(ev string) ([]Line, error) {
	var seen []Line
	want, root := ev, -1
	if err := eachBack(time.Time{}, func(l Line) bool {
		seen = append(seen, l)
		if l.Ev != want {
			return true
		}
		root, want = len(seen)-1, l.Cause
		return want != "" // the first cause: everything after it is seen
	}); err != nil {
		return nil, err
	}
	if root < 0 {
		return nil, fmt.Errorf("no line in rush's log is %s", ev)
	}
	// The furthest cause found is the root, where one before it is gone
	// with an older log.
	seen = seen[:root+1]
	slices.Reverse(seen)
	// What came of the root, through as many steps as it took. A line can
	// be written a little before its cause (each process writes its own),
	// so it's looked at again until nothing more joins.
	parent := parents(seen)
	in := make([]bool, len(seen))
	in[0] = true
	for grew := true; grew; {
		grew = false
		for i := range seen {
			if !in[i] && parent[i] >= 0 && in[parent[i]] {
				in[i], grew = true, true
			}
		}
	}
	var out []Line
	for i, l := range seen {
		if in[i] {
			out = append(out, l)
		}
	}
	return out, nil
}

// evs is where each ev's line is in lines.
func evs(lines []Line) map[string]int {
	at := make(map[string]int, len(lines))
	for i, l := range lines {
		if l.Ev != "" {
			at[l.Ev] = i
		}
	}
	return at
}

// parents is, for each of lines, which line led to it, or -1: its cause,
// else, for a message delivered from the queue, its queuing.
func parents(lines []Line) []int {
	at, queued := evs(lines), map[string]int{}
	for i, l := range lines {
		if l.Kind == "queued" && l.Xid != "" {
			queued[l.Xid] = i
		}
	}
	parent := make([]int, len(lines))
	for i, l := range lines {
		parent[i] = -1
		if j, ok := at[l.Cause]; ok && l.Cause != "" && j != i {
			parent[i] = j
		} else if j, ok := queued[l.Xid]; ok && l.Cause == "" && l.Kind == "delivered" {
			parent[i] = j
		}
	}
	return parent
}

// Node is a line in a tree of causes, and how deep it is.
type Node struct {
	Line
	Depth int
}

// Tree puts lines (oldest first) under their causes: each line after the
// one that led to it, a step deeper. A message delivered from the queue
// goes under its queuing. The lines nothing among them led to are the
// roots, newest first, as a query's are; what came of each is in the
// order it happened.
func Tree(lines []Line) []Node {
	parent := parents(lines)
	kids := make([][]int, len(lines))
	for i, p := range parent {
		if p >= 0 {
			kids[p] = append(kids[p], i)
		}
	}
	out := make([]Node, 0, len(lines))
	shown := make([]bool, len(lines))
	var walk func(i, depth int)
	walk = func(i, depth int) {
		if shown[i] {
			return // two lines that share an ev, each the other's cause
		}
		shown[i] = true
		out = append(out, Node{lines[i], depth})
		for _, k := range kids[i] {
			walk(k, depth+1)
		}
	}
	for i := len(lines) - 1; i >= 0; i-- {
		if parent[i] < 0 {
			walk(i, 0)
		}
	}
	for i := range lines { // a ring of causes has no root: each at the top
		if !shown[i] {
			walk(i, 0)
		}
	}
	return out
}

// Budget is how much text a query prints at most: enough to read, and
// little enough for an agent to take whole.
const Budget = 16 << 10

// Format is lines as text, a line each: how long ago, what, which session,
// which process, its ev and cause, then the rest of it, each value cut
// short. Past budget it stops, saying how many more there were and to
// narrow, and the same if more past them weren't read.
func Format(nodes []Node, now time.Time, budget int, more bool, narrow string) string {
	var b strings.Builder
	for i, n := range nodes {
		row := formatLine(&n.Line, now)
		if d := min(n.Depth, 8); d > 0 {
			row = strings.Repeat("  ", d) + row
		}
		if b.Len()+len(row)+1 > budget {
			fmt.Fprintf(&b, "… %d more; narrow with %s\n", len(nodes)-i, narrow)
			return b.String()
		}
		b.WriteString(row)
		b.WriteByte('\n')
	}
	if more {
		fmt.Fprintf(&b, "… more before these; narrow with %s\n", narrow)
	}
	return b.String()
}

// Flat is lines as nodes at the top, for Format.
func Flat(lines []Line) []Node {
	out := make([]Node, len(lines))
	for i, l := range lines {
		out[i] = Node{Line: l}
	}
	return out
}

// valueMax is how much of a value a row shows, rowMax of the row.
const (
	valueMax = 120
	rowMax   = 320
)

// shownFirst are a line's fields said before the rest, as they say most.
var shownFirst = []string{"why", "err", "from", "xid", "turn"}

// own are the fields every row has a column for.
var own = map[string]bool{"t": true, "kind": true, "proc": true, "pid": true, "session": true, "ev": true, "cause": true}

func formatLine(l *Line, now time.Time) string {
	var b strings.Builder
	b.WriteString(Age(now.Sub(l.At)))
	b.WriteString("  ")
	b.WriteString(l.Kind)
	if l.Session != "" {
		b.WriteString("  s=" + l.Session)
	}
	var m map[string]any
	_ = jsonx.Unmarshal(l.Raw, &m)
	b.WriteString("  " + l.Proc)
	if pid, ok := m["pid"].(float64); ok {
		b.WriteString(":" + strconv.Itoa(int(pid)))
	}
	if l.Ev != "" {
		b.WriteString("  #" + l.Ev)
	}
	if l.Cause != "" {
		b.WriteString(" ←" + l.Cause)
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		if !own[k] && !slices.Contains(shownFirst, k) {
			keys = append(keys, k)
		}
	}
	slices.Sort(keys)
	for _, k := range append(slices.Clone(shownFirst), keys...) {
		v, ok := m[k]
		if !ok || v == nil || v == "" {
			continue
		}
		b.WriteString("  " + k + "=" + cut(value(v), valueMax))
	}
	return cut(b.String(), rowMax)
}

// value is v as a row shows it: a string as it is where it has no spaces,
// else as JSON.
func value(v any) string {
	if s, ok := v.(string); ok && s != "" && !strings.ContainsAny(s, " \t\n\"") {
		return s
	}
	b, err := jsonx.Marshal(v)
	if err != nil {
		return fmt.Sprint(v)
	}
	return string(b)
}

func cut(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n-1]) + "…"
}

// Age is d as a row says it: 40s, 12m, 3h, 2d.
func Age(d time.Duration) string {
	switch {
	case d < time.Minute:
		return strconv.Itoa(max(0, int(d.Seconds()))) + "s"
	case d < time.Hour:
		return strconv.Itoa(int(d.Minutes())) + "m"
	case d < 48*time.Hour:
		return strconv.Itoa(int(d.Hours())) + "h"
	}
	return strconv.Itoa(int(d.Hours()/24)) + "d"
}

// ParseSince is when a --since means: a duration back from now (90s, 10m,
// 2h, 3d) or a time (2026-10-23T09:00:00Z, 2026-10-23).
func ParseSince(s string, now time.Time) (time.Time, error) {
	if s == "" {
		return time.Time{}, nil
	}
	if n, ok := strings.CutSuffix(s, "d"); ok {
		if d, err := strconv.ParseFloat(n, 64); err == nil {
			return now.Add(-time.Duration(d * float64(24*time.Hour))), nil
		}
	}
	if d, err := time.ParseDuration(s); err == nil {
		return now.Add(-d), nil
	}
	for _, layout := range []string{time.RFC3339Nano, "2006-01-02T15:04", "2006-01-02"} {
		if t, err := time.ParseInLocation(layout, s, time.Local); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("%q is neither a while back (10m, 2h, 3d) nor a time (2026-10-23T09:00)", s)
}

// slack is how far out of order the log's lines can be: each process
// writes its own, a little after it logs them.
const slack = time.Minute

// eachBack hands fn rush's log a line at a time, newest first, through
// its last log too, until fn says stop or the lines are from before
// since. A line it hands over stays as it is: fn may keep it.
func eachBack(since time.Time, fn func(Line) bool) error {
	cur := Path()
	for _, p := range []string{cur, cur[:len(cur)-len(".jsonl")] + ".1.jsonl"} {
		done, err := fileBack(p, func(raw []byte) bool {
			l, ok := parseHead(raw)
			if !ok {
				return true
			}
			if !since.IsZero() && !l.At.IsZero() && l.At.Before(since.Add(-slack)) {
				return false
			}
			return fn(l)
		})
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		if done {
			return nil
		}
	}
	return nil
}

// backChunk is how much of a log is read at a time, from its end.
const backChunk = 64 << 10

// fileBack hands fn the lines of the file at p, last first, until it says
// stop: done is whether it did. Each chunk read is a slice of its own,
// never reused, so a line handed over can be kept.
func fileBack(p string, fn func([]byte) bool) (done bool, err error) {
	f, err := os.Open(p)
	if err != nil {
		return false, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return false, err
	}
	var head []byte // the start of a line whose rest was in the chunk after
	for off := st.Size(); off > 0; {
		n := min(backChunk, off)
		off -= n
		b := make([]byte, int(n)+len(head))
		if _, err := f.ReadAt(b[:n], off); err != nil {
			return false, err
		}
		copy(b[n:], head)
		end := len(b)
		for i := len(b) - 1; i >= 0; i-- {
			if b[i] != '\n' {
				continue
			}
			if i+1 < end && !fn(b[i+1:end]) {
				return true, nil
			}
			end = i
		}
		head = b[:end]
	}
	if len(head) > 0 && !fn(head) {
		return true, nil
	}
	return false, nil
}

// parseHead reads the fields of a line that Line has, from the top of its
// object only, skipping the rest without decoding it.
func parseHead(raw []byte) (l Line, ok bool) {
	i := skipSpace(raw, 0)
	if i >= len(raw) || raw[i] != '{' {
		return l, false
	}
	for i++; ; {
		i = skipSpace(raw, i)
		if i >= len(raw) {
			return l, false
		}
		if raw[i] == '}' {
			break
		}
		if raw[i] == ',' {
			i++
			continue
		}
		k, j, ok := str(raw, i)
		if !ok {
			return l, false
		}
		if i = skipSpace(raw, j); i >= len(raw) || raw[i] != ':' {
			return l, false
		}
		if i = skipSpace(raw, i+1); i >= len(raw) {
			return l, false
		}
		if raw[i] != '"' {
			if i = skipValue(raw, i); i < 0 {
				return l, false
			}
			if string(k) == "err" {
				l.Failed = true // an error, not as a string
			}
			continue
		}
		v, j, ok := str(raw, i)
		if !ok {
			return l, false
		}
		i = j
		switch string(k) {
		case "t":
			l.At, _ = time.Parse(time.RFC3339Nano, string(v))
		case "kind":
			l.Kind = string(v)
		case "proc":
			l.Proc = string(v)
		case "session":
			l.Session = string(v)
		case "ev":
			l.Ev = string(v)
		case "cause":
			l.Cause = string(v)
		case "from":
			l.From = string(v)
		case "xid":
			l.Xid = string(v)
		case "spawnedBy":
			l.SpawnedBy = string(v)
		case "err":
			l.Failed = l.Failed || len(v) > 0
		}
	}
	l.Failed = l.Failed || failedKind(l.Kind)
	l.Raw = raw
	return l, true
}

// failedKind is whether a line of kind says something went wrong.
func failedKind(kind string) bool {
	for _, w := range []string{"fail", "kill", "stuck", "forced", "refused", "panic", "crash"} {
		if strings.Contains(kind, w) {
			return true
		}
	}
	return false
}

func skipSpace(b []byte, i int) int {
	for i < len(b) && (b[i] == ' ' || b[i] == '\t' || b[i] == '\r' || b[i] == '\n') {
		i++
	}
	return i
}

// str is the string at b[i] as written, escapes and all, and where it ends.
func str(b []byte, i int) ([]byte, int, bool) {
	if i >= len(b) || b[i] != '"' {
		return nil, 0, false
	}
	for j := i + 1; j < len(b); j++ {
		switch b[j] {
		case '\\':
			j++
		case '"':
			return b[i+1 : j], j + 1, true
		}
	}
	return nil, 0, false
}

// skipValue is where the value at b[i] that isn't a string ends, or -1.
func skipValue(b []byte, i int) int {
	depth := 0
	for i < len(b) {
		switch c := b[i]; c {
		case '"':
			_, j, ok := str(b, i)
			if !ok {
				return -1
			}
			i = j
			continue
		case '{', '[':
			depth++
		case '}', ']':
			if depth == 0 {
				return i
			}
			depth--
		case ',':
			if depth == 0 {
				return i
			}
		}
		i++
	}
	return -1
}
