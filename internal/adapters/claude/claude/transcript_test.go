package claude

import (
	"encoding/json/jsontext"
	"math"
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

const block1 = `{"type":"assistant","timestamp":"2026-09-23T10:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"thinking"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":200}}}}`
const block2 = `{"type":"assistant","timestamp":"2026-09-23T10:00:01Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"see https://github.com/a/b/pull/42"}],"usage":{"input_tokens":10,"output_tokens":50,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":200}}}}`
const user = `{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}`
const next = `{"type":"assistant","timestamp":"2026-09-23T10:01:00Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}`

func TestScanCountsEachMessageOnceAndResumes(t *testing.T) {
	p := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(p, []byte(block1+"\n"+block2+"\n"+user+"\n"+next[:40]), 0o600); err != nil {
		t.Fatal(err)
	}
	var tot Totals
	if _, err := Scan(p, &tot, nil); err != nil {
		t.Fatal(err)
	}
	c, u := tot.Spend()
	// m1 counted once with its final usage: 10 in, 50 out, 1000 read, 200 1h write.
	want := (10*4 + 50*20 + 1000*0.20 + 200*4*2) / 1e6
	if math.Abs(c-want) > 1e-12 || u.Output != 50 {
		t.Fatalf("cost %v want %v, usage %+v", c, want, u)
	}
	if len(tot.PRs) != 1 || tot.PRs[0] != "https://github.com/a/b/pull/42" {
		t.Fatalf("prs %v", tot.PRs)
	}
	// The half-written line is not consumed until it completes.
	f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
	f.WriteString(next[40:] + "\n")
	f.Close()
	if _, err := Scan(p, &tot, nil); err != nil {
		t.Fatal(err)
	}
	c2, u2 := tot.Spend()
	if u2.Output != 51 || c2 <= c {
		t.Fatalf("after resume: %v %+v", c2, u2)
	}
	if d := tot.DaySpend(Day(tot.Last)); math.Abs(d-c2) > 1e-12 {
		t.Fatalf("day spend %v want %v", d, c2)
	}
}

func TestPricing(t *testing.T) {
	if p, _ := PriceFor("claude-opus-5-5"); p.Input != 4 {
		t.Fatalf("opus 5.5 %+v", p)
	}
	if p, _ := PriceFor("claude-opus-5"); p.Input != 5 {
		t.Fatalf("opus 5 %+v", p)
	}
	if p, _ := PriceFor("claude-haiku-4-5-20251001"); p.Input != 1 {
		t.Fatalf("haiku %+v", p)
	}
	if _, ok := PriceFor("gpt-5"); ok {
		t.Fatal("unknown model priced")
	}
	u := TokenUsage{CacheRead: 1e6}
	if c := Cost("claude-sonnet-5", u, false); math.Abs(c-0.2) > 1e-12 {
		t.Fatalf("default cache read is 0.1x input: %v", c)
	}
	if c := Cost("claude-opus-5-5", TokenUsage{Output: 1e6}, true); c != 40 {
		t.Fatalf("fast mode doubles: %v", c)
	}
}

func TestProjectSlug(t *testing.T) {
	if s := ProjectSlug("/Users/lw/Source/github.com/langwatch/langwatch"); s != "-Users-lw-Source-github-com-langwatch-langwatch" {
		t.Fatal(s)
	}
	if s := ProjectSlug("/private/var/folders/zg/3jfp8q9100s_wlx/T/a b"); s != "-private-var-folders-zg-3jfp8q9100s-wlx-T-a-b" {
		t.Fatal(s)
	}
}

func TestScanKeepsTheErrorATurnEndedOn(t *testing.T) {
	limit := `{"type":"assistant","timestamp":"2026-09-23T10:02:00Z","isApiErrorMessage":true,"error":"rate_limit","message":{"id":"x","model":"<synthetic>","content":[{"type":"text","text":"You've hit your session limit · resets 5am (Europe/London)\nmore"}]}}`
	unasked := `{"type":"assistant","timestamp":"2026-09-23T10:03:00Z","isApiErrorMessage":true,"message":{"id":"y","model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}`
	p := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(p, []byte(next+"\n"+limit+"\n"+unasked+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	var tot Totals
	if _, err := Scan(p, &tot, nil); err != nil {
		t.Fatal(err)
	}
	if tot.Halt == nil || tot.Halt.Kind != "rate_limit" || tot.Halt.Text != "You've hit your session limit · resets 5am (Europe/London)" {
		t.Fatalf("halt %+v", tot.Halt)
	}
	// A real reply afterwards means it carried on.
	f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
	f.WriteString(block2 + "\n")
	f.Close()
	if _, err := Scan(p, &tot, nil); err != nil {
		t.Fatal(err)
	}
	if tot.Halt != nil {
		t.Fatalf("halt survived a reply: %+v", tot.Halt)
	}
}

func TestProgressIsTheLastCountThatMoved(t *testing.T) {
	msg := func(text string) jsontext.Value {
		b, _ := jsonx.Marshal([]map[string]string{{"type": "text", "text": text}})
		return b
	}
	for text, want := range map[string]string{
		"Wave done.\n\n**Lint** went 11,065 → **9,052** after W11, typecheck 3,044→2,782.": "Lint went 11,065 → 9,052 after W11, typecheck 3,044→2,782",
		"| api/secrets | 200 -> 403 |":           "",
		"Candidates dropped 206 → 154. Next up.": "Candidates dropped 206 → 154",
		"nothing moved":                          "",
	} {
		if got := progressIn(msg(text)); got != want {
			t.Errorf("%q: got %q want %q", text, got, want)
		}
	}
}

func TestScanCountsCompactions(t *testing.T) {
	compact := `{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}`
	p := filepath.Join(t.TempDir(), "s.jsonl")
	os.WriteFile(p, []byte(block1+"\n"+compact+"\n"+next+"\n"+compact+"\n"), 0o600)
	var tot Totals
	Scan(p, &tot, nil)
	if tot.Compacts != 2 || tot.Context() != 1 {
		t.Fatalf("compacts %d, context %d", tot.Compacts, tot.Context())
	}
}

func TestDirFollowsWritesOverCdAndCwd(t *testing.T) {
	line := func(cwd, content string) string {
		return `{"type":"assistant","cwd":"` + cwd + `","message":{"id":"x","content":[` + content + `]}}`
	}
	bash := func(cmd string) string {
		return `{"type":"tool_use","name":"Bash","input":{"command":"` + cmd + `"}}`
	}
	var tt Totals
	step := func(l, want string) {
		t.Helper()
		consume(&tt, []byte(l))
		if tt.Dir != want {
			t.Fatalf("Dir = %q, want %q", tt.Dir, want)
		}
	}
	step(line("/r/main", `{"type":"text","text":"hi"}`), "/r/main")
	step(line("/r/main", bash("cd /r/pr6900 && git status")), "/r/pr6900")
	step(line("/r/main", bash("ls")), "/r/pr6900")
	step(line("/r/main", bash("cd /tmp/x && ls")), "/r/pr6900")
	step(line("/r/other", `{"type":"text","text":"moved"}`), "/r/other")
	step(line("/r/other", bash(`cd \"/r/q q\"; make`)), "/r/q q")
	step(line("/r/main", `{"type":"tool_use","name":"Write","input":{"file_path":"/Users/me/.claude/projects/p/memory/m.md"}}`), "/r/main")
	step(line("/r/main", `{"type":"tool_use","name":"Edit","input":{"file_path":"/r/wt/a/b.go"}}`), "/r/wt/a")
	// Once it writes, git run from the main checkout doesn't move it back.
	step(line("/r/main", bash("cd /r/main && git log")), "/r/wt/a")
	step(line("/r/other", `{"type":"text","text":"moved"}`), "/r/wt/a")
	step(line("/r/other", `{"type":"tool_use","name":"Edit","input":{"file_path":"/r/wt2/c.go"}},`+bash("cd /r/main")), "/r/wt2")
	// EnterWorktree: the move is known before the next reply.
	step(`{"type":"relocated","sessionId":"s","relocatedCwd":"/r/.claude/worktrees/w"}`, "/r/.claude/worktrees/w")
	step(line("/r/.claude/worktrees/w", `{"type":"text","text":"in"}`), "/r/.claude/worktrees/w")
}
