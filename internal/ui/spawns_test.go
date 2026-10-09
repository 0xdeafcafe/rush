package ui

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"

	_ "github.com/0xdeafcafe/rush/internal/adapters/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A claude -p a session's shell runs is found by the rush session it was
// hosted as (started by this one, while the command ran), or, not hosted,
// by its transcript (begun then, asked the same); drawn under the
// command's row, and listed with the subagents, where it opens like one.
func TestSpawnFollowed(t *testing.T) {
	for _, hosted := range []bool{true, false} {
		spawnFollowed(t, hosted)
	}
}

func spawnFollowed(t *testing.T, hosted bool) {
	cfg := t.TempDir()
	acct := claude.Account{Name: "t", ConfigDir: cfg}
	cwd := "/work/app"
	now := time.Now().UTC().Truncate(time.Second)
	dir := filepath.Join(acct.ProjectsDir(), claude.ProjectSlug(cwd))
	os.MkdirAll(dir, 0o755)
	ts := now.Format(time.RFC3339)
	// Another conversation begun at the same time, asked something else.
	os.WriteFile(filepath.Join(dir, "other.jsonl"), []byte(`{"type":"user","timestamp":"`+ts+`","message":{"role":"user","content":"something else"}}`+"\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "kid.jsonl"), []byte(strings.Join([]string{
		`{"type":"user","timestamp":"` + ts + `","cwd":"/work/app","message":{"role":"user","content":"check the parser for off-by-ones"}}`,
		`{"type":"assistant","timestamp":"` + ts + `","message":{"id":"m1","role":"assistant","model":"claude-sonnet-5","content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/work/app/parse.go"}}]}}`,
		`{"type":"user","timestamp":"` + ts + `","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"r1","content":"…"}]}}`,
	}, "\n")+"\n"), 0o644)

	// Run through rush's stand-in, it's a rush session too.
	t.Setenv("RUSH_HOME", t.TempDir())
	os.MkdirAll(filepath.Join(host.Root(), "kidhost1"), 0o755)
	os.WriteFile(filepath.Join(host.Root(), "kidhost1", "info.json"),
		[]byte(`{"id":"kidhost1","sessionId":"kid","state":"working","hostPid":`+strconv.Itoa(os.Getpid())+`,"startedAt":"`+ts+`","meta":{"spawnedBy":"p1"}}`), 0o644)
	os.WriteFile(filepath.Join(host.Root(), "kidhost1", "config.json"), []byte(`{"id":"kidhost1","kind":"claude","cwd":"/work/app",`+
		`"prompt":"check the parser for off-by-ones","account":{"name":"t","configDir":"`+cfg+`"}}`), 0o644)

	s := convo.New()
	s.Info.Cwd = cwd
	s.Apply(host.Sent{Text: "ask claude"}, now.Add(-time.Second))
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash",
		Input: []byte(`{"command":"claude -p \"check the parser for off-by-ones\"","description":"Parser review"}`)}}}, now.Add(-time.Second))
	c := &hostConn{kind: "claude", key: "k", client: &host.Client{}, sess: s, open: map[string]bool{}}
	key := "b1"
	if hosted {
		c.id, key = "p1", "kidhost1"
	}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{{Key: "k", Acct: acct.Profile()}}}, host: c}

	cmd := m.refreshSpawns()
	if cmd == nil {
		t.Fatal("nothing looked for")
	}
	m.onSpawnFound(cmd().(spawnFoundMsg))
	m.drain(m.refreshSubs()) // what it wrote is read in the background
	r := c.spawns[key]
	if r == nil || r.step != "b1" || filepath.Base(r.path) != "kid.jsonl" {
		t.Fatalf("found %+v", r)
	}
	out := ansi.Strip(joinLines(s.Render(convo.Options{Width: 110, Now: now})))
	for _, w := range []string{"Claude Code  Parser review", "1 step", "parse.go"} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	m.drain(m.refreshSubs())
	if len(c.subs) != 1 || c.subs[0].ID != spawnPrefix+key || c.subs[0].Type != "Claude Code" || c.subs[0].Description != "Parser review" {
		t.Fatalf("subs %+v", c.subs)
	}
	if _, live := c.subState(c.subs[0]); !live {
		t.Error("a spawn whose command runs isn't working")
	}
	m.openSub(c, c.subs[0].ID)
	if c.subTail == nil || len(c.subTail.Sess.Turns) == 0 {
		t.Fatal("opening it shows nothing")
	}
	if hosted && r.hosted != "kidhost1" {
		t.Errorf("its rush session %q: what you type wouldn't reach it", r.hosted)
	}
}

// A subagent run says what it runs on as far as its transcript has told:
// the model it last asked over the one its meta named, its last turn's
// effort, and a spawned agent's provider only where its type doesn't
// name it already.
func TestSubRunsOn(t *testing.T) {
	c := &hostConn{kind: loginsKind, spawns: map[string]*spawnRun{"s1": {kind: "codex"}}}
	s := convo.New()
	s.Requests = []convo.Request{{Model: "claude-sonnet-5"}}
	s.Turns = []*convo.Turn{{Effort: "medium"}, {}}
	if got := c.subRunsOn(convo.Subagent{Type: "Explore", Model: "opus"}, s); got != "Sonnet 5 · medium effort" {
		t.Errorf("read run: %q", got)
	}
	if got := c.subRunsOn(convo.Subagent{Type: "Explore", Model: "opus"}, convo.New()); got != "Opus" {
		t.Errorf("unread run: %q", got)
	}
	if got := c.subRunsOn(convo.Subagent{Type: "Explore"}, convo.New()); got != "" {
		t.Errorf("nothing known: %q", got)
	}
	cs := convo.New()
	cs.Model, cs.Info.Effort = "gpt-5", "high"
	if got := c.subRunsOn(convo.Subagent{ID: spawnPrefix + "s1", Type: "Codex"}, cs); got != "gpt-5 · high effort" {
		t.Errorf("spawned Codex: %q", got)
	}
	if got := c.subRunsOn(convo.Subagent{ID: spawnPrefix + "s1", Type: "fixer"}, cs); got != "Codex · gpt-5 · high effort" {
		t.Errorf("spawned Codex, named otherwise: %q", got)
	}
}

// fanOut is the step the user saw: a shell function wrapping claude -p in
// perl, run three times in the background, its prompt in a variable.
const fanOut = `run() { perl -e 'alarm 2400; exec @ARGV' claude -p --model opus --effort "$1" --allowedTools "Read" "$common YOUR LENS: $2" > "review-$1.md" 2> "review-$1.err"; echo "EXIT $1 $?" >> events.log; }
run high 'correctness' &
run xhigh 'design' &
run medium 'tests' &
wait`

// hostKid writes a session rush hosted for p1's shell, begun at at, and
// the transcript its Claude Code wrote.
func hostKid(t *testing.T, acct claude.Account, id, prompt string, at time.Time) {
	t.Helper()
	d := filepath.Join(host.Root(), id)
	os.MkdirAll(d, 0o755)
	ts := at.Format(time.RFC3339)
	os.WriteFile(filepath.Join(d, "info.json"), []byte(`{"id":"`+id+`","sessionId":"s-`+id+`","state":"stopped","startedAt":"`+ts+`","meta":{"spawnedBy":"p1"}}`), 0o644)
	os.WriteFile(filepath.Join(d, "config.json"), []byte(`{"id":"`+id+`","kind":"claude","cwd":"/work/app","prompt":"`+prompt+
		`","account":{"name":"t","configDir":"`+acct.ConfigDir+`"}}`), 0o644)
	os.WriteFile(acct.TranscriptPath("/work/app", "s-"+id), []byte(strings.Join([]string{
		`{"type":"user","timestamp":"` + ts + `","cwd":"/work/app","message":{"role":"user","content":"` + prompt + `"}}`,
		`{"type":"assistant","timestamp":"` + ts + `","message":{"id":"m-` + id + `","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"reviewed: ` + id + `"}]}}`,
	}, "\n")+"\n"), 0o644)
}

// A step the command text can't read as an agent run (a shell function,
// a perl wrapper, its prompt in a variable) still has the three sessions
// rush hosted for it while it ran as its children, each its own row.
func TestFanOutFollowed(t *testing.T) {
	acct := claude.Account{Name: "t", ConfigDir: t.TempDir()}
	os.MkdirAll(filepath.Join(acct.ProjectsDir(), claude.ProjectSlug("/work/app")), 0o755)
	t.Setenv("RUSH_HOME", t.TempDir())
	now := time.Now().UTC().Truncate(time.Second)
	for i, lens := range []string{"correctness", "design", "tests"} {
		hostKid(t, acct, "kid"+strconv.Itoa(i), "review it. YOUR LENS: "+lens, now.Add(-50*time.Second))
	}
	s := convo.New()
	s.Info.Cwd = "/work/app"
	s.Apply(host.Sent{Text: "three reviews"}, now.Add(-time.Minute))
	s.Apply(headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "b1", Name: "Bash",
		Input: []byte(`{"command":` + strconv.Quote(fanOut) + `,"description":"Three reviews","run_in_background":true}`)}}}, now.Add(-time.Minute))
	s.Apply(headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: "b1", Text: "Command running in background with ID: bg1"}}}, now.Add(-time.Minute))
	s.Apply(headless.TaskStarted{ID: "bg1", ToolUseID: "b1", Type: "local_bash", Description: "Three reviews", Backgrounded: true}, now.Add(-time.Minute))
	if _, ok := s.Step("b1").Spawn(); ok {
		t.Fatal("the command reads as an agent run: this test wants one that doesn't")
	}
	c := &hostConn{kind: "claude", key: "k", id: "p1", client: &host.Client{}, sess: s, open: map[string]bool{}}
	m := &Model{snap: &fleet.Snapshot{Agents: []*fleet.Agent{{Key: "k", Acct: acct.Profile()}}}, host: c}
	m.onSpawnFound(m.refreshSpawns()().(spawnFoundMsg))
	m.drain(m.refreshSubs())
	if kids := c.children("b1"); len(kids) != 3 {
		t.Fatalf("children: %d", len(kids))
	}
	out := ansi.Strip(joinLines(s.Render(convo.Options{Width: 110, Now: now})))
	if n := strings.Count(out, "⇉ Claude Code  review it. YOUR LENS:"); n != 3 {
		t.Errorf("%d agent rows in\n%s", n, out)
	}
	if strings.Contains(out, "perl") {
		t.Errorf("the wrapper shows:\n%s", out)
	}
	for _, sa := range c.spawnSubs() {
		if !strings.HasPrefix(sa.Description, "review it.") {
			t.Errorf("one of three is named for them all: %q", sa.Description)
		}
	}

	// Done, then sent a message (agent_send), one works again: it's running.
	info := filepath.Join(host.Root(), "kid0", "info.json")
	b, _ := os.ReadFile(info)
	os.WriteFile(info, []byte(strings.Replace(string(b), `"state":"stopped"`, `"state":"working","hostPid":`+strconv.Itoa(os.Getpid()), 1)), 0o644)
	s.Apply(headless.TaskDone{ID: "bg1", Status: "completed"}, now.Add(-30*time.Second)) // no step at work lists them all
	c.hostedAt = time.Now().Add(-spawnEvery)                                             // listed since that ended
	m.onSpawnFound(m.refreshSpawns()().(spawnFoundMsg))
	c.runMemo.ok = false // as a read of the runs drops it
	if n := len(c.runningSubs()); n != 1 {
		t.Errorf("%d running after one woke, want 1", n)
	}
}

// A hosted session begun outside every step's window is no step's.
func TestClaimOutsideWindows(t *testing.T) {
	at := time.Now()
	wins := []convo.Window{{Step: "a", Command: "claude -p hi", From: at.Add(-time.Hour), To: at.Add(-50 * time.Minute)}}
	if got := claim(wins, "claude", "", at, false); got != "" {
		t.Errorf("claimed by %q", got)
	}
}

// Two steps whose windows overlap don't both claim a child: the one whose
// command names its program does, else the later to start.
func TestClaimOverlap(t *testing.T) {
	at := time.Now()
	wins := []convo.Window{
		{Step: "named", Command: "run() { claude -p \"$1\"; }; run hi &", From: at.Add(-time.Minute), To: at.Add(time.Minute)},
		{Step: "later", Command: "go test ./...", From: at.Add(-time.Second), To: at.Add(time.Minute)},
	}
	if got := claim(wins, "claude", "", at, false); got != "named" {
		t.Errorf("claimed by %q, want the one naming claude", got)
	}
	wins[0].Command = "make lint"
	if got := claim(wins, "claude", "", at, false); got != "later" {
		t.Errorf("claimed by %q, want the later one", got)
	}
	// While a subagent works, one no step names may be its: none claims it.
	if got := claim(wins, "claude", "", at, true); got != "" {
		t.Errorf("claimed by %q while a subagent worked", got)
	}
}

// A spawn_agent call claims the agent it asked, and only that one, even
// while a shell step that names the program overlaps it.
func TestClaimSpawnTool(t *testing.T) {
	at := time.Now()
	wins := []convo.Window{
		{Step: "shell", Command: "codex exec hi", From: at.Add(-time.Minute), To: at.Add(time.Minute)},
		{Step: "tool-a", Asked: "review the diff", From: at.Add(-2 * time.Second), To: at.Add(time.Minute)},
		{Step: "tool-b", Asked: "write the docs", From: at.Add(-time.Second), To: at.Add(time.Minute)},
	}
	if got := claim(wins, "codex", "review the diff", at, true); got != "tool-a" {
		t.Errorf("claimed by %q, want tool-a", got)
	}
	if got := claim(wins, "codex", "hi", at, false); got != "shell" {
		t.Errorf("a shell run claimed by %q", got)
	}
}
