package fleet

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// pastAgent lists one past session and one running.
type pastAgent struct{ dir string }

func (pastAgent) Kind() agent.Kind   { return "pastfake" }
func (pastAgent) Name() string       { return "Past" }
func (pastAgent) Level() agent.Level { return agent.LevelPreview }
func (pastAgent) Features() map[agent.Feature]agent.Support {
	return map[agent.Feature]agent.Support{}
}
func (a pastAgent) Profiles() []agent.Profile {
	return []agent.Profile{{Kind: "pastfake", Name: "past", Dir: a.dir}}
}
func (pastAgent) Live(agent.Profile) []agent.Session {
	return []agent.Session{{ID: "11111111-live", Cwd: "/repo", UpdatedAt: time.Now()}}
}
func (pastAgent) Past(agent.Profile) []agent.Session {
	return []agent.Session{{ID: "22222222-past", Cwd: "/repo", UpdatedAt: time.Now().Add(-time.Hour)}}
}

// SkipPast leaves past sessions out of a reading, running ones in, and the
// next reading after it's turned off has them again.
func TestSkipPastLeavesPastSessionsOut(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("RUSH_HOME", home+"/rush")
	agent.Register(pastAgent{dir: home + "/past"})

	l := NewLoader(&state.Store{})
	l.Watch(time.Hour)
	count := func() (past, live int) {
		for _, a := range l.Load(true).Agents {
			// Other installed adapters may see live sessions through explicit home overrides.
			if a.Kind != "pastfake" {
				continue
			}
			switch {
			case a.Past:
				past++
			case a.Interactive:
				live++
			}
		}
		return past, live
	}
	l.SkipPast(true)
	if past, live := count(); past != 0 || live != 1 {
		t.Fatalf("with SkipPast: %d past, %d running; want 0, 1", past, live)
	}
	l.SkipPast(false)
	if past, live := count(); past != 1 || live != 1 {
		t.Fatalf("after: %d past, %d running; want 1, 1", past, live)
	}
}

// An agent hidden in the overlay is in no reading, however it's found, and
// the transcript behind it is left alone.
func TestHiddenAgentsAreInNoReading(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("RUSH_HOME", home+"/rush")
	agent.Register(pastAgent{dir: home + "/past"})

	s := &state.Store{}
	l := NewLoader(s)
	keys := func() (out []string) {
		for _, a := range l.Load(true).Agents {
			if a.Kind == "pastfake" {
				out = append(out, a.Key)
			}
		}
		return out
	}
	all := keys()
	if len(all) != 2 {
		t.Fatalf("want a past and a live row, got %v", all)
	}
	s.Hide(all[0])
	if got := keys(); len(got) != 1 || got[0] == all[0] {
		t.Fatalf("hidden %s still listed: %v", all[0], got)
	}
}

// A hidden row is still read, so what counted its subagents is kept from
// one reading to the next rather than made again (reading every run's
// meta file) each time it's looked at.
func TestHiddenRowKeepsItsSubagents(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("RUSH_HOME", home+"/rush")
	s := &state.Store{}
	p := s.Config.ActiveAccount().Profile()
	tr := agent.TranscriptPath(p.Kind, p, "/work/app", "sid1")
	subs := filepath.Join(strings.TrimSuffix(tr, ".jsonl"), "subagents")
	_ = os.MkdirAll(subs, 0o755)
	_ = os.WriteFile(tr, []byte("{}\n"), 0o644)
	_ = os.WriteFile(filepath.Join(subs, "agent-a1.meta.json"), []byte(`{"toolUseId":"t1"}`), 0o644)
	_ = os.WriteFile(filepath.Join(subs, "agent-a1.jsonl"), []byte("{}\n"), 0o644)
	d := filepath.Join(host.Root(), "h1")
	_ = os.MkdirAll(d, 0o755)
	_ = os.WriteFile(filepath.Join(d, "info.json"), []byte(`{"id":"h1","kind":"`+string(p.Kind)+`","sessionId":"sid1","cwd":"/work/app","state":"working","updatedAt":"`+time.Now().Format(time.RFC3339Nano)+`"}`), 0o644)

	l := NewLoader(s)
	key := state.Key(p.Name, "a:h1")
	l.Load(true)
	e, ok := l.subs[key]
	if !ok || e.runs == nil || e.st.Spawned != 1 {
		t.Fatalf("its runs weren't counted: %+v", e)
	}
	s.Hide(key)
	for _, a := range l.Load(true).Agents {
		if a.Key == key {
			t.Fatal("a hidden row is listed")
		}
	}
	if got, ok := l.subs[key]; !ok || got.runs != e.runs {
		t.Fatal("a hidden row's subagent counter was dropped")
	}
}
