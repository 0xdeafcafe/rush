package remote

import (
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// gitRepo makes a repository with one commit, then a change to it and a new
// file, in a session's folder of this machine.
func gitRepo(t *testing.T) string {
	t.Helper()
	repo := realDir(t.TempDir())
	run := func(args ...string) {
		c := exec.Command("git", append([]string{"-C", repo, "-c", "user.email=t@t", "-c", "user.name=t"}, args...)...)
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	run("init", "-q")
	os.WriteFile(filepath.Join(repo, "a.txt"), []byte("one\n"), 0o600)
	run("add", ".")
	run("commit", "-qm", "first")
	os.WriteFile(filepath.Join(repo, "a.txt"), []byte("one\ntwo\n"), 0o600)
	os.WriteFile(filepath.Join(repo, "new.txt"), []byte("new\n"), 0o600)
	os.MkdirAll(filepath.Join(repo, "sub"), 0o700)
	dir := filepath.Join(host.Root(), "9e9e9e9e")
	os.MkdirAll(dir, 0o700)
	b, _ := jsonx.Marshal(host.Info{ID: "9e9e9e9e", Cwd: filepath.Join(repo, "sub"), State: "idle", UpdatedAt: time.Now()})
	if err := os.WriteFile(filepath.Join(dir, "info.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	return repo
}

func TestCheckGit(t *testing.T) {
	newServer(t)
	repo := gitRepo(t)
	sub := filepath.Join(repo, "sub")
	for _, c := range []struct {
		name string
		r    GitRequest
		ok   bool
	}{
		{"diff in the session's folder", GitRequest{sub, []string{"diff", "HEAD", "--numstat", "-z"}}, true},
		{"the repository's top, above it", GitRequest{repo, []string{"rev-parse", "--show-toplevel"}}, true},
		{"an untracked file in it", GitRequest{repo, []string{"diff", "--no-index", "--", "/dev/null", filepath.Join(repo, "new.txt")}}, true},
		{"a range", GitRequest{repo, []string{"log", "HEAD~1..HEAD"}}, true},
		{"a folder no session has", GitRequest{"/etc", []string{"status"}}, false},
		{"a command that writes", GitRequest{repo, []string{"commit", "-m", "x"}}, false},
		{"diff into a file", GitRequest{repo, []string{"diff", "--output=/tmp/x"}}, false},
		{"config on the line", GitRequest{repo, []string{"log", "-c", "core.pager=sh"}}, false},
		{"an external diff", GitRequest{repo, []string{"diff", "--ext-diff"}}, false},
		{"a file outside", GitRequest{repo, []string{"diff", "--no-index", "--", "/dev/null", "/etc/passwd"}}, false},
		{"a file outside, relatively", GitRequest{repo, []string{"diff", "--no-index", "--", "/dev/null", "../../../../etc/passwd"}}, false},
		{"no command", GitRequest{repo, nil}, false},
	} {
		if _, err := checkGit(c.r); (err == nil) != c.ok {
			t.Errorf("%s: %v", c.name, err)
		}
	}
}

// The view's changes view of an attached session reads the remote's tree,
// through serve, with paths only ever under the unreachable folder here.
func TestWorkingTreeOfAnAttachedSession(t *testing.T) {
	s := newServer(t)
	repo := gitRepo(t)
	ts := httptest.NewServer(s.Handler())
	defer ts.Close()
	if err := SaveConfig(Config{Peers: []Peer{{Name: "box", URL: ts.URL, Token: s.Token}}}); err != nil {
		t.Fatal(err)
	}
	convo.RemoteGit = Git
	defer func() { convo.RemoteGit = nil }()
	local := filepath.Join(unreachable("box"), repo, "sub")
	var tree *convo.Tree
	for end := time.Now().Add(10 * time.Second); time.Now().Before(end); time.Sleep(50 * time.Millisecond) {
		if tree = convo.WorkingTree(local); tree.Err != "reading git…" {
			break
		}
	}
	if tree.Err != "" || tree.Root != filepath.Join(unreachable("box"), repo) || len(tree.Files) != 2 {
		t.Fatalf("tree: %+v", tree)
	}
	for _, f := range tree.Files {
		if !strings.HasPrefix(f.Path, unreachable("box")+"/") {
			t.Errorf("a path of this machine: %s", f.Path)
		}
		if strings.HasSuffix(f.Path, "a.txt") && (f.Add != 1 || f.Del != 0) || strings.HasSuffix(f.Path, "new.txt") && !f.Untracked {
			t.Errorf("file %+v", f)
		}
	}
	// any other folder is this machine's own git
	if _, handled, _ := Git(repo, "status"); handled {
		t.Error("a local folder went to the remote")
	}
}
