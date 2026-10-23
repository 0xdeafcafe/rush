package events

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"
)

// writeLog puts lines in a test's rush log as they'd be kept, oldest first.
func writeLog(t *testing.T, lines ...string) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("RUSH_HOME", home)
	if err := os.WriteFile(Path(), []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
}

func at(ago time.Duration) string { return time.Now().Add(-ago).UTC().Format(time.RFC3339Nano) }

func TestLinesCarryEvAndTheProcessCause(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	SetCause("abc12345")
	defer SetCause("")
	Log("first", nil)
	Log("second", map[string]any{"ev": "00000001", "cause": "feedface"})
	Log("third", map[string]any{"cause": ""})
	Flush()
	lines, _, err := Query(Filter{})
	if err != nil || len(lines) != 3 {
		t.Fatalf("lines %v, err %v", lines, err)
	}
	third, second, first := lines[0], lines[1], lines[2]
	if len(first.Ev) != 8 || first.Cause != "abc12345" {
		t.Fatalf("the process's cause goes on its lines: %+v", first)
	}
	if second.Ev != "00000001" || second.Cause != "feedface" {
		t.Fatalf("a line's own ev and cause are kept: %+v", second)
	}
	if third.Cause != "" || strings.Contains(string(third.Raw), "cause") {
		t.Fatalf("a line that names no cause has none: %s", third.Raw)
	}
}

func TestQueryFilters(t *testing.T) {
	writeLog(t,
		`{"ev":"a1","kind":"host-started","proc":"host","session":"s1","t":"`+at(3*time.Hour)+`"}`,
		`{"ev":"a2","kind":"delivered","proc":"host","session":"s1","t":"`+at(20*time.Minute)+`","text":"fix the Auth bug"}`,
		`{"ev":"a3","kind":"queued","proc":"host","session":"s2","t":"`+at(10*time.Minute)+`"}`,
		`not json`,
		`{"ev":"a4","kind":"plugin-start","info":{"kind":"x","session":"s1"},"proc":"plugind","t":"`+at(time.Minute)+`"}`,
		`{"ev":"a5","kind":"plugin-stop","proc":"plugind","t":"`+at(30*time.Second)+`"}`,
	)
	evs := func(f Filter) string {
		lines, more, err := Query(f)
		if err != nil {
			t.Fatal(err)
		}
		var out []string
		for _, l := range lines {
			out = append(out, l.Ev)
		}
		if more {
			out = append(out, "+")
		}
		return strings.Join(out, " ")
	}
	for _, c := range []struct {
		f    Filter
		want string
	}{
		{Filter{}, "a5 a4 a3 a2 a1"},
		{Filter{Limit: 2}, "a5 a4 +"},
		{Filter{Sessions: []string{"s1"}}, "a2 a1"}, // not a4, whose session is inside another field
		{Filter{Kinds: []string{"delivered", "queued"}}, "a3 a2"},
		{Filter{Kinds: []string{"plugin*"}}, "a5 a4"},
		{Filter{Proc: "plugind"}, "a5 a4"},
		{Filter{Since: time.Now().Add(-15 * time.Minute)}, "a5 a4 a3"},
		{Filter{Grep: "auth"}, "a2"},
	} {
		if got := evs(c.f); got != c.want {
			t.Errorf("%+v: got %q, want %q", c.f, got, c.want)
		}
	}
}

func TestQueryReadsBackAcrossChunksAndTheLastLog(t *testing.T) {
	var lines []string
	for i := range 3000 { // past a chunk, so lines straddle chunks
		lines = append(lines, fmt.Sprintf(`{"ev":"n%d","kind":"k","pad":"%s","t":"%s"}`, i, strings.Repeat("x", 40), at(time.Duration(3000-i)*time.Second)))
	}
	writeLog(t, lines...)
	old := Path()[:len(Path())-len(".jsonl")] + ".1.jsonl"
	if err := os.WriteFile(old, []byte(`{"ev":"old","kind":"k","t":"`+at(5*time.Hour)+`"}`+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	all, _, err := Query(Filter{})
	if err != nil || len(all) != 3001 || all[0].Ev != "n2999" || all[2999].Ev != "n0" || all[3000].Ev != "old" {
		t.Fatalf("read %d lines, err %v", len(all), err)
	}
	for i, l := range all[:3000] {
		if l.Ev != fmt.Sprintf("n%d", 2999-i) {
			t.Fatalf("line %d is %s", i, l.Ev)
		}
	}
}

func TestFormatKeepsToItsBudget(t *testing.T) {
	now := time.Now()
	var lines []Line
	for i := range 50 {
		raw := fmt.Sprintf(`{"ev":"e%d","kind":"state","pid":7,"proc":"host","session":"s1","why":"%s","t":"x"}`, i, strings.Repeat("long ", 100))
		l, _ := parseHead([]byte(raw))
		l.At = now.Add(-time.Duration(i) * 90 * time.Second)
		lines = append(lines, l)
	}
	out := Format(Flat(lines), now, 2000, false, "-k")
	if len(out) > 2000+80 {
		t.Fatalf("%d chars past a budget of 2000", len(out))
	}
	rows := strings.Split(strings.TrimSpace(out), "\n")
	if last := rows[len(rows)-1]; !strings.HasPrefix(last, "… ") || !strings.HasSuffix(last, " more; narrow with -k") {
		t.Fatalf("it ends saying what's left: %q", last)
	}
	if !strings.HasPrefix(rows[0], "0s  state  s=s1  host:7  #e0") || !strings.Contains(rows[0], "why=\"long long") || !strings.Contains(rows[0], "…") {
		t.Fatalf("row: %q", rows[0])
	}
	if !strings.HasPrefix(rows[1], "1m  ") {
		t.Fatalf("ages: %q", rows[1])
	}
}

func TestAge(t *testing.T) {
	for d, want := range map[time.Duration]string{40 * time.Second: "40s", 12 * time.Minute: "12m", 3 * time.Hour: "3h", 50 * time.Hour: "2d"} {
		if got := Age(d); got != want {
			t.Errorf("Age(%v) = %s, want %s", d, got, want)
		}
	}
}

// A story across two sessions: A's agent sends to B, which was asleep;
// the send wakes B's host, whose agent starts and ends a turn. A message
// queued in B is delivered later with no cause, by its exchange.
func story(t *testing.T) {
	writeLog(t,
		`{"ev":"aS","kind":"agent-started","proc":"host","session":"A","t":"`+at(50*time.Minute)+`"}`,
		`{"ev":"zz","kind":"state","proc":"host","session":"C","t":"`+at(45*time.Minute)+`"}`,
		`{"ev":"hS","cause":"wk","kind":"host-started","proc":"host","session":"B","t":"`+at(40*time.Minute)+`"}`,
		`{"ev":"wk","cause":"aS","kind":"woken","proc":"session","session":"B","t":"`+at(40*time.Minute-time.Second)+`"}`,
		`{"ev":"dl","cause":"wk","from":"A","kind":"delivered","proc":"host","session":"B","turn":1,"xid":"x1","t":"`+at(39*time.Minute)+`"}`,
		`{"ev":"q1","cause":"aS","from":"A","kind":"queued","proc":"host","session":"B","xid":"x2","t":"`+at(38*time.Minute)+`"}`,
		`{"ev":"te","cause":"dl","kind":"turn-end","proc":"host","session":"B","turn":1,"t":"`+at(30*time.Minute)+`"}`,
		`{"ev":"d2","from":"A","kind":"delivered","proc":"host","session":"B","turn":2,"xid":"x2","t":"`+at(29*time.Minute)+`"}`,
		`{"ev":"gf","kind":"plugind-exit","err":"broker died","proc":"plugind","t":"`+at(20*time.Minute)+`"}`,
		`{"ev":"ok","kind":"saved","proc":"ui","t":"`+at(10*time.Minute)+`"}`,
	)
}

func shape(nodes []Node) string {
	var b strings.Builder
	for _, n := range nodes {
		b.WriteString(strings.Repeat(".", n.Depth) + n.Ev + " ")
	}
	return strings.TrimSpace(b.String())
}

func TestWhyIndentsUnderCauses(t *testing.T) {
	story(t)
	lines, err := Why("B", nil, time.Now().Add(-time.Hour))
	if err != nil {
		t.Fatal(err)
	}
	// A's agent is in it as B's sender and the cause of the wake; C isn't
	// linked; the broker's failure is rush-wide; the ui's save isn't.
	// The host started is written before the wake that caused it, and
	// still goes under it.
	if got, want := shape(Tree(lines)), "gf aS .wk ..hS ..dl ...te .q1 ..d2"; got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
	out := Format(Tree(lines), time.Now(), Budget, false, "--last")
	if !strings.Contains(out, "\n  ") || !strings.Contains(out, "#wk ←aS") {
		t.Fatalf("formatted:\n%s", out)
	}
}

func TestChainFollowsOneLine(t *testing.T) {
	story(t)
	lines, err := Chain("te")
	if err != nil {
		t.Fatal(err)
	}
	if got, want := shape(Tree(lines)), "aS .wk ..hS ..dl ...te .q1 ..d2"; got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
	if _, err := Chain("nope"); err == nil {
		t.Fatal("an ev that's in no line is an error")
	}
}

func TestParseSince(t *testing.T) {
	now := time.Date(2026, 10, 23, 12, 0, 0, 0, time.UTC)
	for in, want := range map[string]time.Time{
		"10m":                  now.Add(-10 * time.Minute),
		"3d":                   now.Add(-72 * time.Hour),
		"2026-10-23T09:00:00Z": time.Date(2026, 10, 23, 9, 0, 0, 0, time.UTC),
	} {
		if got, err := ParseSince(in, now); err != nil || !got.Equal(want) {
			t.Errorf("%s: %v, %v", in, got, err)
		}
	}
	if _, err := ParseSince("soon", now); err == nil {
		t.Error("soon isn't a time")
	}
}
