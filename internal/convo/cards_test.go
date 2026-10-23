package convo

import (
	"reflect"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/host"
)

func bashStep(cmd, stdout, stderr string, status Status) *Step {
	return &Step{Tool: "Bash", Status: status, Input: raw(map[string]any{"command": cmd}),
		Result: raw(map[string]any{"stdout": stdout, "stderr": stderr})}
}

func TestCommitCards(t *testing.T) {
	heredoc := "git add -A && git commit -m \"$(cat <<'EOF'\nperf(menubar): the app wakes only while agents are working\n\nThe menubar polled every second even when nothing\nwas running; it now sleeps until a session reports work.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nEOF\n)\" && git log --oneline -1"
	for _, c := range []struct {
		name, cmd, out string
		want           []card
	}{
		{"heredoc", heredoc,
			"[main 3225847] perf(menubar): the app wakes only while agents are working\n 2 files changed, 40 insertions(+), 12 deletions(-)\n3225847 perf(menubar): the app wakes only while agents are working\n",
			[]card{{kind: "commit", branch: "main", sha: "3225847", subject: "perf(menubar): the app wakes only while agents are working",
				body:  []string{"The menubar polled every second even when nothing", "was running; it now sleeps until a session reports work."},
				files: 2, add: 40, del: 12, with: []string{"Claude"}}}},
		{"two -m, root, insertions only", `git commit -m "init" -m "the \"first\" one"`,
			"[main (root-commit) 1a2b3c4] init\n 1 file changed, 3 insertions(+)\n create mode 100644 a.go\n",
			[]card{{kind: "commit", branch: "main", sha: "1a2b3c4", subject: "init", body: []string{`the "first" one`}, files: 1, add: 3, root: true}}},
		{"amend, detached, -F", "git commit --amend -F /tmp/msg.txt 2>&1 | tail -4",
			"[detached HEAD 60df9b5e08] fix(identity): tell people what was recorded\n Date: Mon Sep 22 10:00:00 2026 +0100\n 9 files changed, 275 insertions(+), 62 deletions(-)\nRebasing (120/148)\rRebasing (121/148)",
			[]card{{kind: "commit", branch: "detached HEAD", sha: "60df9b5e08", subject: "fix(identity): tell people what was recorded", files: 9, add: 275, del: 62, amend: true}}},
		{"two commits in a chain", `git commit -m "a" a.go && git commit -m "b" b.go`,
			"[main 1111111] a\n 1 file changed, 1 deletion(-)\n[main 2222222] b\n 1 file changed, 1 insertion(+)\n",
			[]card{{kind: "commit", branch: "main", sha: "1111111", subject: "a", files: 1, del: 1}, {kind: "commit", branch: "main", sha: "2222222", subject: "b", files: 1, add: 1}}},
		{"quiet, -F - from a heredoc, log after", "git add a.go && git commit -q -F - <<'EOF'\nfix(ui): hovering lights a row\n\nOnly a click selects it.\nEOF\ngit add b.go && git commit --quiet -m \"feat: b\" >/dev/null\ngit log --oneline -3",
			"ca5fad6 (HEAD -> main) feat: b\nd01e709 fix(ui): hovering lights a row\n4d467c4 feat(convo): older\n",
			[]card{{kind: "commit", sha: "d01e709", subject: "fix(ui): hovering lights a row", body: []string{"Only a click selects it."}},
				{kind: "commit", sha: "ca5fad6", subject: "feat: b"}}},
		{"a message that mentions -m", "git commit -q -F - <<'EOF'\nfix: reads more than -m's messages\n\nOnly -m's were read.\nEOF",
			"", []card{{kind: "commit", subject: "fix: reads more than -m's messages", body: []string{"Only -m's were read."}}}},
		{"a script that mentions a commit", "python3 - <<'EOF'\ns = \"git commit -q -F - <<'EOF'\\nfix: x\"\nEOF\ngo test ./...",
			"ok  \tgithub.com/x/y\t1.2s\n", nil},
		{"quiet, no log", `git commit -q -m "x"`, "", []card{{kind: "commit", subject: "x"}}},
		{"nothing to commit", `git commit -m "x"`, "On branch main\nnothing to commit, working tree clean\n", nil},
		{"not a commit", "cat notes.txt", "[main 3225847] looks like one\n", nil},
	} {
		got := cardsOf(bashStep(c.cmd, c.out, "", OK))
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s:\n got %+v\nwant %+v", c.name, got, c.want)
		}
	}
}

func TestPushCard(t *testing.T) {
	got := cardsOf(bashStep("git push -u origin feat/cards && git push --force origin main", "",
		"remote: \nremote: Create a pull request for 'feat/cards' on GitHub by visiting:\nTo github.com:0xdeafcafe/rush.git\n * [new branch]      feat/cards -> feat/cards\n + 88c42b1...3225847 main -> main (forced update)\n ! [rejected]        dev -> dev (fetch first)\nbranch 'feat/cards' set up to track 'origin/feat/cards'.\n", Failed))
	want := []card{{kind: "push", remote: "github.com/0xdeafcafe/rush", refs: []pushed{
		{from: "feat/cards", to: "feat/cards", span: "new branch"},
		{from: "main", to: "main", span: "88c42b1...3225847", forced: true},
	}}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("push\n got %+v\nwant %+v", got, want)
	}
	if got := cardsOf(bashStep("git push", "", "Everything up-to-date\n", OK)); got != nil {
		t.Errorf("a push that moved nothing is no card, got %+v", got)
	}
	for u, want := range map[string]string{
		"git@github.com:o/r.git":             "github.com/o/r",
		"https://github.com/o/r.git":         "github.com/o/r",
		"https://x-token:abc@github.com/o/r": "github.com/o/r",
		"ssh://git@gitlab.com:22/o/r.git":    "gitlab.com:22/o/r",
		"github.com:o/r.git":                 "github.com/o/r",
		"/tmp/bare.git":                      "/tmp/bare",
	} {
		if got := repoName(u); got != want {
			t.Errorf("repoName(%q) = %q, want %q", u, got, want)
		}
	}
}

func TestPRCards(t *testing.T) {
	cmd := "git push -u origin feat/cards && gh pr create --base main --title \"feat(convo): commits render as cards\" --body \"$(cat <<'EOF'\n## Summary\n- a commit shows as a card\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\nEOF\n)\""
	got := cardsOf(bashStep(cmd, "https://github.com/0xdeafcafe/rush/pull/142\n", "Creating pull request for feat/cards into main in 0xdeafcafe/rush\n\n", OK))
	want := []card{{kind: "pr", state: "open", num: "142", url: "github.com/0xdeafcafe/rush/pull/142", title: "feat(convo): commits render as cards",
		head: "feat/cards", base: "main", body: []string{"## Summary", "- a commit shows as a card"}}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("pr create\n got %+v\nwant %+v", got, want)
	}
	if got := cardsOf(bashStep("gh pr create --fill", "", "a pull request for branch \"x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/7\n", Failed)); got != nil {
		t.Errorf("an existing pull request isn't a new one, got %+v", got)
	}
	got = cardsOf(bashStep("gh pr merge 142 --squash --delete-branch", "", "", OK))
	want = []card{{kind: "pr", state: "merged", num: "142", base: "squashed"}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("pr merge, quiet\n got %+v\nwant %+v", got, want)
	}
	got = cardsOf(bashStep("gh pr merge --rebase", "", "✓ Rebased and merged pull request 0xdeafcafe/rush#142 (feat: cards)\n✓ Deleted local branch feat/cards and switched to branch main\n", OK))
	want = []card{{kind: "pr", state: "merged", num: "142", base: "rebased", title: "feat: cards", head: "feat/cards"}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("pr merge, told\n got %+v\nwant %+v", got, want)
	}
}

// A commit folded into a run of clean steps still shows its card, and the
// row doesn't repeat what the card says.
func TestCardUnderFoldedRun(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	cmd := "git add -A && git commit -m \"$(cat <<'EOF'\nfeat: cards\n\nA commit shows what it was.\nEOF\n)\""
	for i, e := range []any{
		host.Sent{Text: "commit it"},
		toolUse("b1", "Bash", map[string]any{"command": cmd, "description": "Commit the cards"}),
		toolResult("b1", "", false, map[string]any{"stdout": "[main abc1234] feat: cards\n 3 files changed, 20 insertions(+), 5 deletions(-)\n", "stderr": ""}),
		toolUse("b2", "Bash", map[string]any{"command": "go build ./..."}),
		toolResult("b2", "", false, map[string]any{"stdout": "", "stderr": ""}),
		toolUse("b3", "Bash", map[string]any{"command": "git status --short"}),
		toolResult("b3", "", false, map[string]any{"stdout": "", "stderr": ""}),
		say("Committed."),
		headless.Result{Subtype: "success"},
	} {
		s.Apply(e, at(i))
	}
	out := plain(s.Render(Options{Width: 100, Now: at(20)}))
	for _, w := range []string{"▸ 3 steps  git commit, go build, git status", "● abc1234 · main", "+20 −5 · 3 files", "│ feat: cards", "│ A commit shows what it was."} {
		if !strings.Contains(out, w) {
			t.Errorf("missing %q in\n%s", w, out)
		}
	}
	out = plain(s.Render(Options{Width: 100, Now: at(20), Open: map[string]bool{"t1:run:1": true}}))
	if n := strings.Count(out, "● abc1234"); n != 1 {
		t.Errorf("an opened run should draw the card once, under its step; drew it %d times:\n%s", n, out)
	}
	if strings.Contains(out, "“feat: cards”") {
		t.Errorf("the row shouldn't repeat the card's subject:\n%s", out)
	}
	// Picked, a card shows what its key does.
	if out := plain(s.Render(Options{Width: 100, Now: at(20), Selected: EntityRef + "commit:abc1234"})); !strings.Contains(out, "↵ copy abc1234") {
		t.Errorf("picked card hides its key:\n%s", out)
	}
	// Too narrow for a card: the row alone.
	if out := plain(s.Render(Options{Width: 24, Now: at(20)})); strings.Count(out, "╭") > 1 { // the one box is your message
		t.Errorf("no card at width 24:\n%s", out)
	}
}

func TestBuildCards(t *testing.T) {
	for _, c := range []struct {
		name, cmd, out string
		want           []card
	}{
		{"go build", "go build ./... 2>&1 | tail",
			"# github.com/x/convo\ninternal/convo/shell.go:448:6: redirRe redeclared in this block\n\tinternal/convo/shell.go:40:2: other declaration of redirRe\n",
			[]card{{kind: "build", what: "go build", failed: 1, fails: []failure{{msg: "shell.go:448: redirRe redeclared in this block"}}}}},
		{"go install", "go install ./cmd/rush", "# x\ncmd/rush/main.go:12:2: undefined: foo\n",
			[]card{{kind: "build", what: "go install", failed: 1, fails: []failure{{msg: "main.go:12: undefined: foo"}}}}},
		{"tsc", "pnpm exec tsc --noEmit",
			"src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/lib/b.ts:9:1 - error TS2304: Cannot find name 'x'.\n\nFound 2 errors in 2 files.\n",
			[]card{{kind: "build", what: "tsc", failed: 2, fails: []failure{{msg: "a.ts:3: Type 'string' is not assignable to type 'number'."}, {msg: "b.ts:9: Cannot find name 'x'."}}}}},
		{"vite", "pnpm build",
			"✘ [ERROR] Expected \";\" but found \"}\"\n\n    src/main.ts:4:2:\n      4 │   }\n",
			[]card{{kind: "build", what: "pnpm build", failed: 1, fails: []failure{{msg: "main.ts:4: Expected \";\" but found \"}\""}}}}},
		{"webpack", "npm run start", "ERROR in ./src/app.tsx 12:4\nModule not found: Error: Can't resolve './x'\n",
			[]card{{kind: "build", what: "npm run start", failed: 1, fails: []failure{{msg: "app.tsx:12: Module not found: Error: Can't resolve './x'"}}}}},
		{"nx", "npx nx run-many -t build",
			" NX   Ran target build for 3 projects (4s)\n\n   ✖  1/3 failed\n\n Failed tasks:\n\n - web:build\n",
			[]card{{kind: "build", what: "nx", failed: 1, fails: []failure{{name: "web:build", build: true}}}}},
		{"a build that went is no card", "go build ./...", "", nil},
		{"a test build failure is the tests card's", "go build ./... && go test ./internal/convo",
			"# github.com/x/convo\ninternal/convo/shell.go:448:6: redirRe redeclared in this block\nFAIL\tgithub.com/x/convo [build failed]\n",
			[]card{{kind: "tests", what: "test", failed: 1, fails: []failure{{name: "convo", msg: "shell.go:448: redirRe redeclared in this block", build: true}}}}},
	} {
		got := cardsOf(bashStep(c.cmd, c.out, "", Failed))
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s:\n got %+v\nwant %+v", c.name, got, c.want)
		}
	}
}

func TestTestCards(t *testing.T) {
	for _, c := range []struct {
		name, cmd, out string
		want           []card
	}{
		{"go", "go test ./... 2>&1 | tail -30",
			"--- FAIL: TestRender (0.00s)\n    --- FAIL: TestRender/wide (0.00s)\n        convo_test.go:141: missing \"x\" in\n            ╭─ card\n            │ body\n--- FAIL: TestFold (0.01s)\n    convo_test.go:170: first turn should fold\nFAIL\nFAIL\tgithub.com/x/convo\t0.2s\nok  \tgithub.com/x/ui\t0.1s\n",
			[]card{{kind: "tests", what: "test", failed: 2, fails: []failure{{name: "TestRender/wide", msg: "convo_test.go:141: missing \"x\" in\n    ╭─ card\n    │ body"}, {name: "TestFold", msg: "convo_test.go:170: first turn should fold"}}}}},
		{"go -v", "go test -v -run TestFold ./internal/convo",
			"=== RUN   TestFold\n    convo_test.go:170: first turn should fold\n--- FAIL: TestFold (0.01s)\n=== RUN   TestOther\n--- PASS: TestOther (0.00s)\nFAIL\n",
			[]card{{kind: "tests", what: "test", failed: 1, passed: 1, fails: []failure{{name: "TestFold", msg: "convo_test.go:170: first turn should fold"}}}}},
		{"go build", "go test ./internal/convo",
			"# github.com/x/convo\ninternal/convo/shell.go:448:6: redirRe redeclared in this block\n\tinternal/convo/shell.go:40:2: other declaration of redirRe\nFAIL\tgithub.com/x/convo [build failed]\n",
			[]card{{kind: "tests", what: "test", failed: 1, fails: []failure{{name: "convo", msg: "shell.go:448: redirRe redeclared in this block", build: true}}}}},
		{"pytest", "uv run pytest -q",
			"..F.\nFAILED tests/test_api.py::test_login - AssertionError: 401 != 200\n==== 1 failed, 3 passed, 1 skipped in 0.52s ====\n",
			[]card{{kind: "tests", what: "test", failed: 1, passed: 3, skipped: 1, fails: []failure{{name: "tests/test_api.py::test_login", msg: "AssertionError: 401 != 200"}}}}},
		{"vitest", "npx vitest run",
			" ❯ src/a.test.ts (41 tests | 1 failed) 20ms\n   ✓ cards > draws a push 1ms\n   × cards > draws a commit 3ms\n     → expected 'a' to be 'b'\n\n⎯⎯ Failed Tests 1 ⎯⎯\n\n \x1b[31mFAIL\x1b[39m  src/a.test.ts > cards > draws a commit\nAssertionError: expected 'a' to be 'b'\n      Tests  1 failed | 40 passed (41)\n",
			[]card{{kind: "tests", what: "test", failed: 1, passed: 40, fails: []failure{{name: "src/a.test.ts > cards > draws a commit", msg: "AssertionError: expected 'a' to be 'b'"}}}}},
		{"vitest, tail cut the names", "pnpm exec vitest run src/a.test.ts 2>&1 | tail -5",
			"     → expected 200 to be 404\n ❯ src/a.test.ts (9 tests | 2 failed) 2.1s\n Test Files  1 failed (1)\n      Tests  2 failed | 7 passed (9)\n",
			[]card{{kind: "tests", what: "test", failed: 2, passed: 7, fails: []failure{{name: "src/a.test.ts", msg: "2 of 9 failed"}}}}},
		{"jest", "npx jest",
			"  ● cards › draws a commit\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 4 passed, 5 total\n",
			[]card{{kind: "tests", what: "test", failed: 1, passed: 4, fails: []failure{{name: "cards > draws a commit", msg: "expect(received).toBe(expected)"}}}}},
		{"cargo", "cargo test",
			"test tests::a ... ok\ntest tests::b ... FAILED\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured\n",
			[]card{{kind: "tests", what: "test", failed: 1, passed: 1, fails: []failure{{name: "tests::b"}}}}},
		{"passing is no card", "go test ./...", "ok  \tgithub.com/x/convo\t0.2s\n", nil},
		{"not a test run", "grep -rn FAILED logs/", "FAILED tests/x.py::y\n", nil},
	} {
		got := cardsOf(bashStep(c.cmd, c.out, "", Failed))
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s:\n got %+v\nwant %+v", c.name, got, c.want)
		}
	}
}

func TestMergeCards(t *testing.T) {
	for _, c := range []struct {
		name, cmd, out string
		want           []card
	}{
		{"fast-forward", "git merge feat/cards", "Updating 88c42b1..3225847\nFast-forward\n a.go | 2 +-\n 1 file changed, 1 insertion(+), 1 deletion(-)\n",
			[]card{{kind: "merge", what: "merge", branch: "feat/cards", how: "fast-forward", files: 1, add: 1, del: 1}}},
		{"conflict", `git merge --no-ff -m "merge it" feat`, "Auto-merging a.go\nCONFLICT (content): Merge conflict in a.go\nCONFLICT (modify/delete): b.go deleted in feat and modified in HEAD.\nAutomatic merge failed; fix conflicts and then commit the result.\n",
			[]card{{kind: "merge", what: "merge", branch: "feat", conflicts: []string{"a.go", "b.go deleted in feat and modified in HEAD."}}}},
		{"rebase", "git rebase origin/main", "Successfully rebased and updated refs/heads/feat/cards.\n",
			[]card{{kind: "merge", what: "rebase", branch: "origin/main", head: "feat/cards", how: "rebased"}}},
		{"rebase stopped", "git rebase main", "Auto-merging a.go\nCONFLICT (content): Merge conflict in a.go\nerror: could not apply 1a2b3c4... feat: cards\nhint: Resolve all conflicts manually\n",
			[]card{{kind: "merge", what: "rebase", branch: "main", sha: "1a2b3c4", subject: "feat: cards", conflicts: []string{"a.go"}}}},
		{"up to date", "git pull", "Already up to date.\n", nil},
	} {
		got := cardsOf(bashStep(c.cmd, c.out, "", OK))
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s:\n got %+v\nwant %+v", c.name, got, c.want)
		}
	}
}

func TestDiscardCards(t *testing.T) {
	for _, c := range []struct {
		name, cmd, out string
		want           []card
		tone           tone
	}{
		{"reset --hard", "git reset --hard origin/main", "HEAD is now at 3225847 perf(menubar): sleep\n",
			[]card{{kind: "discard", what: "reset --hard", sha: "3225847", subject: "perf(menubar): sleep", undo: "git reset --hard HEAD@{1}"}}, toneLost},
		{"reset --soft is fine", "git reset --soft HEAD~1", "", nil, 0},
		{"branch -D", "git branch -D feat/a feat/b", "Deleted branch feat/a (was 1a2b3c4d).\nDeleted branch feat/b (was 5e6f7a8).\n",
			[]card{{kind: "discard", what: "branch -D", branch: "feat/a", sha: "1a2b3c4d", undo: "git branch feat/a 1a2b3c4"}, {kind: "discard", what: "branch -D", branch: "feat/b", sha: "5e6f7a8", undo: "git branch feat/b 5e6f7a8"}}, toneLost},
		{"branch -d", "git branch -d feat", "Deleted branch feat (was 1a2b3c4).\n",
			[]card{{kind: "discard", what: "branch -d", branch: "feat", sha: "1a2b3c4", warn: true, undo: "git branch feat 1a2b3c4"}}, toneWarn},
		{"stash", "git stash", "Saved working directory and index state WIP on main: 3225847 perf: sleep\n",
			[]card{{kind: "discard", what: "stash", subject: "WIP on main: 3225847 perf: sleep", warn: true, undo: "git stash pop"}}, toneWarn},
		{"stash pop is no card", "git stash pop", "On branch main\nDropped refs/stash@{0} (9f1c2ab0d)\n", nil, 0},
		{"stash drop", "git stash drop", "Dropped refs/stash@{0} (9f1c2ab0d)\n",
			[]card{{kind: "discard", what: "stash drop", branch: "refs/stash@{0}", sha: "9f1c2ab0d", undo: "git stash apply 9f1c2ab"}}, toneLost},
		{"clean", "git clean -fd", "Removing tmp/\nRemoving scratch.go\n",
			[]card{{kind: "discard", what: "clean", lost: []string{"tmp/", "scratch.go"}}}, toneLost},
		{"clean dry run", "git clean -nd", "Would remove tmp/\n", nil, 0},
		{"checkout --", "git checkout -- a.go b.go", "Updated 2 paths from the index\n",
			[]card{{kind: "discard", what: "checkout", lost: []string{"a.go", "b.go"}}}, toneLost},
		{"checkout a branch", "git checkout -b feat", "Switched to a new branch 'feat'\n", nil, 0},
		{"restore", "git restore .", "", []card{{kind: "discard", what: "restore", lost: []string{"."}}}, toneLost},
		{"unstage", "git restore --staged a.go", "", nil, 0},
	} {
		got := cardsOf(bashStep(c.cmd, c.out, "", OK))
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s:\n got %+v\nwant %+v", c.name, got, c.want)
			continue
		}
		for _, x := range got {
			if x.tone() != c.tone {
				t.Errorf("%s: tone %d, want %d", c.name, x.tone(), c.tone)
			}
		}
	}
}

func TestCardTones(t *testing.T) {
	for _, c := range []struct {
		card card
		want tone
	}{
		{card{kind: "commit"}, toneMade},
		{card{kind: "commit", amend: true}, toneWarn},
		{card{kind: "pr", state: "open"}, toneMade},
		{card{kind: "pr", state: "draft"}, toneNone},
		{card{kind: "push", refs: []pushed{{span: "a..b"}}}, toneNone},
		{card{kind: "push", refs: []pushed{{span: "a..b"}, {span: "new branch"}}}, toneMade},
		{card{kind: "push", refs: []pushed{{span: "new branch"}, {span: "a...b", forced: true}}}, toneWarn},
		{card{kind: "push", refs: []pushed{{span: "a...b", forced: true}, {span: "deleted"}}}, toneLost},
		{card{kind: "merge", how: "fast-forward"}, toneNone},
		{card{kind: "merge", conflicts: []string{"a.go"}}, toneWarn},
		{card{kind: "tests", failed: 1}, toneLost},
	} {
		if got := c.card.tone(); got != c.want {
			t.Errorf("%+v: tone %d, want %d", c.card, got, c.want)
		}
	}
}

// A run whose tests failed reads as failed, though tail made it exit 0, and
// doesn't fold into a clean run.
func TestFailedTestsRow(t *testing.T) {
	s := New()
	s.Info.Cwd = "/work"
	out := "--- FAIL: TestFold (0.01s)\n    convo_test.go:170: missing \"▸ 2 steps\" in\n        ▸ 3 steps: git add, go build\n--- FAIL: TestPush (0.00s)\n    cards_test.go:55: push\nFAIL\nFAIL\tgithub.com/x/convo\t0.2s\n"
	for i, e := range []any{
		host.Sent{Text: "test it"},
		toolUse("b1", "Bash", map[string]any{"command": "go build ./..."}),
		toolResult("b1", "", false, map[string]any{"stdout": "", "stderr": ""}),
		toolUse("b2", "Bash", map[string]any{"command": "go test ./... 2>&1 | tail -30", "description": "Run the tests"}),
		toolResult("b2", "", false, map[string]any{"stdout": out, "stderr": ""}),
		toolUse("b3", "Bash", map[string]any{"command": "git status --short"}),
		toolResult("b3", "", false, map[string]any{"stdout": "", "stderr": ""}),
		say("Two fail."),
		headless.Result{Subtype: "success"},
	} {
		s.Apply(e, at(i))
	}
	got := plain(s.Render(Options{Width: 100, Now: at(20)}))
	for _, w := range []string{"▸ 3 steps  go build, tests, git status  ✗ 1 failed", "✗ 2 failed", "TestFold", "  convo_test.go:170 missing \"▸ 2 steps\" in", "      ▸ 3 steps: git add, go build", "  cards_test.go:55 push"} {
		if !strings.Contains(got, w) {
			t.Errorf("missing %q in\n%s", w, got)
		}
	}
	if strings.Contains(got, "all ok") {
		t.Errorf("failed tests folded into a clean run:\n%s", got)
	}
}

// A commit, its push and the pull request for its branch read as one card.
func TestJoinedCards(t *testing.T) {
	commit := card{kind: "commit", sha: "abc1234", branch: "feat/cards", subject: "feat: cards"}
	push := card{kind: "push", remote: "github.com/o/r", refs: []pushed{{from: "feat/cards", to: "feat/cards", span: "new branch"}}}
	other := card{kind: "commit", sha: "def5678", branch: "main", subject: "chore: other"}
	pr := card{kind: "pr", state: "open", num: "142", head: "feat/cards", title: "feat: cards"}
	got := joinCards([]card{commit, push, other, pr})
	if len(got) != 2 || got[0].sha != "def5678" || got[1].kind != "pr" || len(got[1].joined) != 1 || got[1].pushed != "github.com/o/r" {
		t.Fatalf("joined %+v", got)
	}
	// A pull request naming no branch takes its commits' push.
	pr.head = ""
	got = joinCards([]card{commit, push, pr})
	if len(got) != 1 || got[0].pushed != "github.com/o/r" || len(got[0].joined) != 1 {
		t.Errorf("unnamed branch %+v", got)
	}
	// A commit alone takes its push in.
	got = joinCards([]card{commit, push})
	if len(got) != 1 || got[0].kind != "commit" || got[0].pushed == "" {
		t.Errorf("commit and push %+v", got)
	}
}

// A failure filtered out of its log by grep doesn't take on the lines of
// what ran before it.
func TestGoFailureKeepsToItsLog(t *testing.T) {
	got := cardsOf(bashStep("git log --oneline -3; go test ./... 2>&1 | grep -E '^(--- FAIL|FAIL)'", "303edcf fix(session): resume\na0331e8 Set up once\n--- FAIL: TestA (0.01s)\n--- FAIL: TestB (0.02s)\nFAIL\n", "", OK))
	if len(got) != 1 || len(got[0].fails) != 2 || got[0].fails[0].msg != "" {
		t.Errorf("got %+v", got)
	}
}
