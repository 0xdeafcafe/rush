package fleet

import (
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fswait"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A cache that looked at a watched file is trusted until the file changes,
// a sweep is due, or a load can't tell what changed; one that looked
// before a change isn't, whichever load it's asked in.
func TestFresh(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	f := filepath.Join(t.TempDir(), "t.jsonl")
	_ = os.WriteFile(f, []byte("a\n"), 0o644)
	l := NewLoader(&state.Store{})
	l.Watch(time.Hour)
	l.watching.w = fswait.NewWatcher()
	defer l.watching.w.Close()
	now := time.Now()
	load := func() { l.drain(now); l.began(now) }

	load() // the first: can't tell, sweeps
	l.watching.w.Watch([]string{f})
	at := l.checked()
	if l.fresh(f, at) {
		t.Fatal("fresh in a sweep")
	}
	load() // told f is newly watched: it may have changed before
	if l.fresh(f, at) {
		t.Fatal("trusted across being newly watched")
	}
	at = l.checked()
	load()
	if !l.fresh(f, at) {
		t.Fatal("a quiet watched file isn't fresh")
	}
	fh, _ := os.OpenFile(f, os.O_APPEND|os.O_WRONLY, 0)
	_, _ = fh.WriteString("b\n")
	fh.Close()
	load()
	if l.fresh(f, at) {
		t.Fatal("trusted after a write")
	}
	load() // the write was told a load ago: still not, for a cache that didn't look
	if l.fresh(f, at) {
		t.Fatal("a write missed by a load that didn't look at it")
	}
	at = l.checked()
	load()
	if !l.fresh(f, at) {
		t.Fatal("looked at since the write, it isn't fresh")
	}
	if l.quick = true; l.fresh(f, at) {
		t.Fatal("fresh in a quick load")
	}
	l.quick = false
	now = now.Add(sweepEvery)
	load()
	if l.fresh(f, at) {
		t.Fatal("fresh in a sweep")
	}
	if l.fresh(filepath.Join(filepath.Dir(f), "unwatched"), at) {
		t.Fatal("an unwatched file is fresh")
	}
}

// A stopped host started again by another process is seen at the next
// load: its info.json is renamed in, which its watched folder tells.
func TestHostRestartSeen(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	d := filepath.Join(host.Root(), "rs")
	_ = os.MkdirAll(d, 0o755)
	write := func(state string, pid int) {
		tmp := filepath.Join(d, "info.json.tmp")
		_ = os.WriteFile(tmp, []byte(`{"id":"rs","kind":"unknown-test","state":"`+state+`","hostPid":`+strconv.Itoa(pid)+`,"updatedAt":"`+time.Now().Format(time.RFC3339Nano)+`"}`), 0o644)
		_ = os.Rename(tmp, filepath.Join(d, "info.json"))
	}
	write("stopped", 0)
	l := NewLoader(&state.Store{})
	l.Watch(time.Hour)
	stateOf := func() string {
		for _, in := range l.hosts.List() {
			if in.ID == "rs" {
				return in.State
			}
		}
		return ""
	}
	for range 3 { // watching, told what's newly watched, trusted
		l.Load(true)
	}
	if !l.fresh(d, l.hosts.At) {
		t.Fatal("the host's folder isn't trusted: this test wants it to be")
	}
	write("working", os.Getpid())
	l.Load(true)
	if got := stateOf(); got != "working" {
		t.Fatalf("restarted host reads %q", got)
	}
}

// A row with no subagents folder, rewound to a conversation that has one
// in the same (quiet) project, counts the new one's runs.
func TestSubagentsFollowRewind(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	proj := t.TempDir()
	t1, t2 := filepath.Join(proj, "s1.jsonl"), filepath.Join(proj, "s2.jsonl")
	_ = os.WriteFile(t1, []byte("{}\n"), 0o644)
	_ = os.WriteFile(t2, []byte("{}\n"), 0o644)
	subs := filepath.Join(proj, "s2", "subagents")
	_ = os.MkdirAll(subs, 0o755)
	_ = os.WriteFile(filepath.Join(subs, "agent-a1.meta.json"), []byte(`{"toolUseId":"t1"}`), 0o644)
	_ = os.WriteFile(filepath.Join(subs, "agent-a1.jsonl"), []byte("{}\n"), 0o644)

	l := NewLoader(&state.Store{})
	l.Watch(time.Hour)
	l.watching.w = fswait.NewWatcher()
	defer l.watching.w.Close()
	now := time.Now()
	load := func() { l.drain(now); l.began(now) }
	for range 3 { // looked at, told newly watched, trusted
		load()
		l.subagents(agent.LegacyKind, "k", t1, false, now)
		l.watching.w.Watch(l.subWatch)
	}
	if !l.fresh(proj, l.subs["k"].chk) {
		t.Fatal("the project folder isn't trusted: this test wants it to be")
	}
	load()
	if st, _ := l.subagents(agent.LegacyKind, "k", t2, false, now); st.Spawned != 1 {
		t.Fatalf("rewound to a conversation with a run: %+v", st)
	}
}

// A transcript found is trusted while it's watched and quiet; removed, it
// isn't found there again.
func TestTranscriptOfTrusted(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("kqueue only")
	}
	acct := t.TempDir()
	pr := agent.Profile{Kind: agent.LegacyKind, Name: "t", Dir: acct}
	at := agent.TranscriptPath(pr.Kind, pr, "/work/app", "sid1")
	_ = os.MkdirAll(filepath.Dir(at), 0o755)
	_ = os.WriteFile(at, []byte("{}\n"), 0o644)
	l := NewLoader(&state.Store{})
	l.Watch(time.Hour)
	l.watching.w = fswait.NewWatcher()
	defer l.watching.w.Close()
	now := time.Now()
	for range 3 { // looked at, told newly watched, trusted
		l.drain(now)
		l.began(now)
		if got := l.transcriptOf(pr, "/work/app", "sid1"); got != at {
			t.Fatalf("found %q, want %q", got, at)
		}
		l.watching.w.Watch(l.subWatch)
	}
	if !l.fresh(at, l.found[foundKey{pr.Kind, pr.Name, pr.Dir, "/work/app", "sid1"}].at) {
		t.Fatal("not trusted: this test wants it to be")
	}
	_ = os.Remove(at)
	l.drain(now)
	if _, ok := l.found[foundKey{pr.Kind, pr.Name, pr.Dir, "/work/app", "sid1"}]; ok && l.fresh(at, l.found[foundKey{pr.Kind, pr.Name, pr.Dir, "/work/app", "sid1"}].at) {
		t.Fatal("a removed transcript is still trusted")
	}
}
