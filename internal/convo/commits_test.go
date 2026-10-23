package convo

import (
	"os/exec"
	"reflect"
	"strings"
	"testing"
	"time"
)

func sh(t *testing.T, dir, script string) string {
	t.Helper()
	cmd := exec.Command("sh", "-c", script)
	cmd.Dir = dir
	cmd.Env = append(cmd.Environ(), "GIT_AUTHOR_NAME=a", "GIT_AUTHOR_EMAIL=a@b", "GIT_COMMITTER_NAME=a", "GIT_COMMITTER_EMAIL=a@b", "GIT_CONFIG_GLOBAL=/dev/null")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%s: %v\n%s", script, err, out)
	}
	return string(out)
}

// cardsNow is what a drawer shows for st once git has answered.
func cardsNow(t *testing.T, dir string, st *Step) []card {
	t.Helper()
	s := &Session{}
	s.Info.Cwd = dir
	d := &drawer{s: s}
	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
		if cs, ok := d.gitCommits(st); ok {
			return withCommits(cardsOf(st), cs)
		}
		lookups.Lock()
		l := lookups.m[func() commitQuery { q, _ := d.commitQuery(st); return q }()]
		done := l != nil && l.done
		lookups.Unlock()
		if done {
			return cardsOf(st) // git couldn't say
		}
	}
	t.Fatal("git never answered")
	return nil
}

func TestGitCommits(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	dir := t.TempDir()
	sh(t, dir, "git init -q -b main && echo a > a && git add a && git commit -qm init")

	// The screenshot's: a loop of quiet commits, each message a variable.
	cmd := "for m in one two; do echo $m > $m; git add $m; msg=\"feat: $m\"; C=\"Co-Authored-By: Claude <c@d>\"; git commit -q -m \"$msg\" -m \"body of $m\" -m \"$C\"; done"
	time.Sleep(time.Until(time.Now().Truncate(time.Second).Add(time.Second))) // a second of its own
	start := time.Now()
	sh(t, dir, cmd)
	st := bashStep(cmd, "", "", OK)
	st.Start, st.End = start, time.Now()
	got := cardsNow(t, dir, st)
	if len(got) != 2 {
		t.Fatalf("want two cards, got %+v", got)
	}
	for i, m := range []string{"one", "two"} {
		c := got[i]
		c.sha = ""
		want := card{kind: "commit", branch: "main", subject: "feat: " + m, body: []string{"body of " + m}, with: []string{"Claude"}, files: 1, add: 1}
		if !reflect.DeepEqual(c, want) {
			t.Errorf("commit %d:\n got %+v\nwant %+v", i, c, want)
		}
	}

	// Printed, amended: git's message, not the command's.
	cmd = "git commit --amend -F /tmp/whatever"
	start = time.Now()
	out := sh(t, dir, "echo x >> two && git add two && git commit --amend -m 'feat: two, again' -m 'longer'")
	st = bashStep(cmd, out, "", OK)
	st.Start, st.End = start.Add(-3*time.Second), time.Now() // takes in the loop's too: what git printed is what counts
	got = cardsNow(t, dir, st)
	if len(got) != 1 || got[0].subject != "feat: two, again" || !reflect.DeepEqual(got[0].body, []string{"longer"}) || !got[0].amend || got[0].branch != "main" {
		t.Errorf("amend: got %+v", got)
	}
	if head := strings.TrimSpace(sh(t, dir, "git rev-parse HEAD")); got[0].sha != head {
		t.Errorf("amend: sha %s, HEAD is %s", got[0].sha, head)
	}

	// Somewhere git can't be asked, the command's reading stands.
	st = bashStep(`git commit -q -m "x"`, "", "", OK)
	st.Start, st.End = start, time.Now()
	if got := cardsNow(t, t.TempDir()+"/gone", st); !reflect.DeepEqual(got, []card{{kind: "commit", subject: "x"}}) {
		t.Errorf("no repo: got %+v", got)
	}
}

func TestCommitShellMessage(t *testing.T) {
	// Only the shell knew what "$msg" was; better no card than "$msg".
	for _, cmd := range []string{`git commit -q -m "$msg" -m "$C"`, "git commit -q -m \"$(cat msg.txt)\"", "git commit -qm `cat m`"} {
		if got := cardsOf(bashStep(cmd, "", "", OK)); got != nil {
			t.Errorf("%s: got %+v", cmd, got)
		}
	}
	// Written as it reads: single quotes, a quoted heredoc.
	for _, cmd := range []string{`git commit -q -m 'costs $5'`, "git commit -q -m \"$(cat <<'EOF'\ncosts $5\nEOF\n)\""} {
		if got := cardsOf(bashStep(cmd, "", "", OK)); len(got) != 1 || got[0].subject != "costs $5" {
			t.Errorf("%s: got %+v", cmd, got)
		}
	}
}

// mergeNow is the merge card a drawer shows for st once git has answered.
func mergeNow(t *testing.T, dir string, st *Step) card {
	t.Helper()
	s := &Session{}
	s.Info.Cwd = dir
	d := &drawer{s: s}
	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
		if g, ok := d.gitMerge(st); ok {
			cs := withMerge(cardsOf(st), &g)
			for i := range cs {
				if cs[i].kind == "merge" {
					return cs[i]
				}
			}
		}
	}
	t.Fatal("git never answered")
	return card{}
}

func TestGitMerge(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	dir := t.TempDir()
	sh(t, dir, "git init -q -b main && git commit -q --allow-empty -m init && git checkout -q -b feat && "+
		"for i in 1 2 3 4 5 6; do git commit -q --allow-empty -m \"feat: $i\"; done && "+
		"git checkout -q main && git commit -q --allow-empty -m 'main moved' && git checkout -q -b ff feat~4")

	time.Sleep(time.Until(time.Now().Truncate(time.Second).Add(time.Second)))
	start := time.Now()
	out := sh(t, dir, "git checkout -q main && git merge --no-edit feat")
	st := bashStep("git merge --no-edit feat", out, "", OK)
	st.Start, st.End = start, time.Now()
	c := mergeNow(t, dir, st)
	if c.made == "" || c.into != "main" || c.cameN != 6 || len(c.came) != 5 || c.came[0].subject != "feat: 6" || c.was.subject != "main moved" || c.forked {
		t.Errorf("merge commit: got %+v", c)
	}

	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: 120, Open: map[string]bool{}}, cw: 120}
	d.card(c, 2)
	// Folded, it says how much came in; opened, what.
	if drawn := stripANSI(d.lines[0].Text); len(d.lines) != 1 || !strings.Contains(drawn, "6 commits") {
		t.Errorf("folded merge: %q", drawn)
	}
	d.lines, d.opened = nil, true
	d.card(c, 2)
	got := make([]string, 0, len(d.lines))
	for _, l := range d.lines {
		got = append(got, strings.TrimRight(stripANSI(l.Text), " "))
	}
	drawn := strings.Join(got, "\n")
	for _, want := range []string{"⇣ merged feat", "│ ●─╮ " + c.made[:7] + "  merge commit", "│ │ ● " + c.came[0].sha[:7] + "  feat: 6", "│ │ ┊ +2 more", "│ ● │ ", "╰ into main"} {
		if !strings.Contains(drawn, want) {
			t.Errorf("drawn merge lacks %q:\n%s", want, drawn)
		}
	}
	if strings.Contains(drawn, "· merge commit") {
		t.Errorf("the graph says it's a merge commit; the head needn't:\n%s", drawn)
	}

	// A fast-forward is one lane, from where the branch was.
	time.Sleep(time.Until(time.Now().Truncate(time.Second).Add(time.Second)))
	start = time.Now()
	out = sh(t, dir, "git checkout -q ff && git merge feat~2")
	st = bashStep("git merge feat~2", out, "", OK)
	st.Start, st.End = start, time.Now()
	c = mergeNow(t, dir, st)
	if c.made != "" || c.cameN != 2 || c.came[0].subject != "feat: 4" || c.was.subject != "feat: 2" || c.into != "ff" {
		t.Errorf("fast-forward: got %+v", c)
	}
}
