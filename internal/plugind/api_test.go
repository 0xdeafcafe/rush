package plugind

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func testPlugin(t *testing.T, m plugin.Manifest) (*runner, plugin.Plugin) {
	t.Helper()
	home, _ := filepath.EvalSymlinks(t.TempDir())
	t.Setenv("RUSH_HOME", home)
	if m.Name == "" {
		m.Name = "kanban"
	}
	_ = os.MkdirAll(plugin.DataDir(m.Name), 0o700)
	r := newRunner(nil, m.Name, "")
	return r, plugin.Plugin{Manifest: m, Dir: filepath.Join(plugin.Root(), m.Name)}
}

func denied(err error) bool {
	var e *plugin.Error
	return errors.As(err, &e) && e.Code == plugin.CodeDenied
}

// writeInfo makes a session as its host would publish it.
func writeInfo(t *testing.T, i host.Info) {
	t.Helper()
	d := filepath.Join(host.Root(), i.ID)
	_ = os.MkdirAll(d, 0o700)
	b, _ := jsonx.Marshal(i)
	if err := os.WriteFile(filepath.Join(d, "info.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestExec(t *testing.T) {
	ws, _ := filepath.EvalSymlinks(t.TempDir())
	r, p := testPlugin(t, plugin.Manifest{Workspaces: []string{ws},
		Exec: map[string][]string{"echo": {"/bin/sh", "-c", `echo "$@"; cat; pwd; exit 3`, "sh"}}})
	ctx := context.Background()

	res, err := r.exec(ctx, p, "echo", []string{"list", "--json"}, "in", ws)
	if err != nil {
		t.Fatal(err)
	}
	out := res.(map[string]any)
	if out["code"] != 3 || out["stdout"] != "list --json\nin"+ws+"\n" {
		t.Fatalf("exec = %#v", out)
	}
	if _, err := r.exec(ctx, p, "rm", nil, "", ""); !denied(err) {
		t.Fatalf("a program the manifest doesn't name ran: %v", err)
	}
	if _, err := r.exec(ctx, p, "echo", nil, "", "/"); !denied(err) {
		t.Fatalf("ran outside its workspaces: %v", err)
	}
	res, err = r.exec(ctx, p, "echo", nil, "", "")
	if err != nil || !strings.Contains(res.(map[string]any)["stdout"].(string), plugin.DataDir(p.Name)) {
		t.Fatalf("without a cwd it should run in its data folder: %v %v", res, err)
	}
}

func TestQueueable(t *testing.T) {
	ws, _ := filepath.EvalSymlinks(t.TempDir())
	r, p := testPlugin(t, plugin.Manifest{Workspaces: []string{ws}, Sessions: []string{plugin.CapQueue}})
	elsewhere, _ := filepath.EvalSymlinks(t.TempDir())
	writeInfo(t, host.Info{ID: "mine", Cwd: elsewhere, StartedBy: p.Name})
	writeInfo(t, host.Info{ID: "inside", Cwd: ws, PermissionMode: "acceptEdits"})
	writeInfo(t, host.Info{ID: "outside", Cwd: elsewhere})
	writeInfo(t, host.Info{ID: "yolo", Cwd: ws, PermissionMode: "bypassPermissions"})
	for id, ok := range map[string]bool{"mine": true, "inside": true, "outside": false, "yolo": false} {
		err := r.queueable(p, id)
		if ok != (err == nil) || !ok && !denied(err) {
			t.Errorf("queueable(%s) = %v", id, err)
		}
	}
	if err := r.queueable(p, "../x"); err == nil {
		t.Error("a path was taken for a session id")
	}
}

func TestStartRefuses(t *testing.T) {
	ws, _ := filepath.EvalSymlinks(t.TempDir())
	r, p := testPlugin(t, plugin.Manifest{Workspaces: []string{ws}, Sessions: []string{plugin.CapStart}})
	cases := map[string]startReq{
		"meta key":      {meta: map[string]string{"bad key": "x"}},
		"meta value":    {meta: map[string]string{"card": strings.Repeat("x", 2000)}},
		"branch option": {worktree: &worktreeReq{Branch: "--upload-pack=evil"}},
		"base option":   {worktree: &worktreeReq{Base: "-b"}},
		"branch dots":   {worktree: &worktreeReq{Branch: "a/../b"}},
		"not a repo":    {worktree: &worktreeReq{Branch: "task"}},
	}
	for name, q := range cases {
		q.cwd, q.prompt = ws, "hi"
		if _, err := r.start(p, q); err == nil {
			t.Errorf("%s: started", name)
		}
	}
}

func TestGitOf(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	repo, _ := filepath.EvalSymlinks(t.TempDir())
	git := func(args ...string) {
		t.Helper()
		if out, err := exec.Command("git", append([]string{"-C", repo}, args...)...).CombinedOutput(); err != nil {
			t.Fatalf("git %v: %s", args, out)
		}
	}
	git("init", "-q", "-b", "main")
	git("-c", "user.email=a@b", "-c", "user.name=a", "commit", "-q", "--allow-empty", "-m", "x")
	wt := filepath.Join(repo, ".claude", "worktrees", "task")
	git("worktree", "add", "-q", "-b", "task-1", wt)
	_ = os.MkdirAll(filepath.Join(wt, "sub"), 0o700)

	if g := gitOf(repo); g.repo != repo || g.branch != "main" || g.worktree {
		t.Errorf("main checkout = %+v", g)
	}
	if g := gitOf(filepath.Join(wt, "sub")); g.repo != wt || g.branch != "task-1" || !g.worktree {
		t.Errorf("worktree = %+v", g)
	}
}

func TestSame(t *testing.T) {
	a := Session{ID: "a", State: "idle", Meta: map[string]string{"card": "1"}}
	b := a
	b.UpdatedAt = b.UpdatedAt.Add(1)
	b.Meta = map[string]string{"card": "1"}
	if !same(a, b) {
		t.Error("a new look alone counted as a change")
	}
	b.ContextTokens = 5
	if same(a, b) {
		t.Error("a change was missed")
	}
}
