package remote

import (
	"bytes"
	"cmp"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// GitRequest is one read-only git command in a session's folder.
type GitRequest struct {
	Cwd  string   `json:"cwd"`
	Args []string `json:"args"`
}

// GitReply is what it printed, or why it didn't run.
type GitReply struct {
	Out   []byte `json:"out"`
	Error string `json:"error,omitempty"`
}

// gitReads are the git commands a remote client may run: the ones rush's
// changes view reads the working tree and its commits with.
var gitReads = []string{"rev-parse", "diff", "ls-files", "reflog", "show", "name-rev", "rev-list", "merge-base", "log", "status"}

// gitRefused are options that would write, run something, or look outside
// the folder.
var gitRefused = []string{"--output", "--ext-diff", "--textconv", "--exec-path", "--git-dir", "--work-tree", "--upload-pack", "--receive-pack", "-c", "-C", "--config"}

const gitOutMost = 32 << 20

// checkGit is whether r may run here: in a folder at or under one of this
// machine's sessions' (or its repository's top, as rush's own changes view
// asks), a read from gitReads, with no refused option and no path, given
// whole or relative, outside that session's repository.
func checkGit(r GitRequest) (string, error) {
	if !filepath.IsAbs(r.Cwd) || len(r.Args) == 0 || !slices.Contains(gitReads, r.Args[0]) {
		return "", errors.New("that git command isn't available remotely")
	}
	for _, a := range r.Args[1:] {
		for _, bad := range gitRefused {
			if a == bad || strings.HasPrefix(a, bad+"=") || len(bad) == 2 && strings.HasPrefix(a, bad) {
				return "", fmt.Errorf("git %s isn't available remotely", a)
			}
		}
	}
	cwd := realDir(r.Cwd)
	var root string
	for _, i := range host.List() {
		if i.IsRemote() || i.Cwd == "" {
			continue
		}
		sc := realDir(i.Cwd)
		if under(cwd, sc) || under(sc, cwd) && gitTop(sc) == cwd {
			root = cmp.Or(gitTop(sc), sc)
			break
		}
	}
	if root == "" {
		return "", errors.New("no session works in that folder")
	}
	for _, a := range r.Args[1:] {
		if strings.HasPrefix(a, "-") || a == "/dev/null" {
			continue
		}
		p := a // a path, or a revision, which reads as one inside
		if !filepath.IsAbs(p) {
			p = filepath.Join(cwd, p)
		}
		if !under(p, root) {
			return "", fmt.Errorf("%s is outside the session's repository", a)
		}
	}
	return cwd, nil
}

// realDir is p with its links resolved, as git names folders.
func realDir(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return filepath.Clean(p)
}

func under(p, root string) bool {
	rel, err := filepath.Rel(filepath.Clean(root), filepath.Clean(p))
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

func gitTop(dir string) string {
	out, err := runGit(dir, "rev-parse", "--show-toplevel")
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// runGit runs git in dir as a look: no locks taken, no fsmonitor hook or
// external diff run, giving up after 10 seconds.
func runGit(dir string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", append([]string{"-c", "core.fsmonitor=false", "-c", "diff.external=", "-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_OPTIONAL_LOCKS=0", "GIT_TERMINAL_PROMPT=0")
	var out bytes.Buffer
	cmd.Stdout = &limitWriter{w: &out, n: gitOutMost}
	err := cmd.Run()
	return out.Bytes(), err
}

type limitWriter struct {
	w io.Writer
	n int
}

func (l *limitWriter) Write(p []byte) (int, error) {
	n := len(p)
	if len(p) > l.n {
		p = p[:l.n]
	}
	l.n -= len(p)
	_, err := l.w.Write(p)
	return n, err
}

func (s *Server) git(w http.ResponseWriter, r *http.Request) {
	var req GitRequest
	if err := readJSON(r, &req); err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	dir, err := checkGit(req)
	if err != nil {
		fail(w, http.StatusForbidden, err.Error())
		return
	}
	out, err := runGit(dir, req.Args...)
	rep := GitReply{Out: out}
	if err != nil {
		rep.Error = err.Error()
	}
	writeJSON(w, rep)
}

// Git runs git for this machine's rush view in a folder it shows an attached
// session working in (see unreachable), on that session's machine, through
// its serve: the changes view of a remote session reads the remote's tree.
// handled is false for any other folder. Paths cross back and forth under
// the unreachable folder, so what the view gets back still names no path
// of this machine.
func Git(dir string, args ...string) (out []byte, handled bool, err error) {
	top := filepath.Join(state.Dir(), "remote-cwd")
	if abs, e := filepath.Abs(top); e == nil {
		top = abs
	}
	rel, e := filepath.Rel(top, dir)
	if e != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return nil, false, nil
	}
	machine, _, _ := strings.Cut(rel, string(filepath.Separator))
	cfg, e := LoadConfig()
	if e != nil {
		return nil, true, e
	}
	p, ok := cfg.Peer(machine)
	if !ok {
		return nil, true, fmt.Errorf("%s isn't a machine in remote.json", machine)
	}
	local := unreachable(machine)
	strip := func(s string) string {
		if s == local {
			return "/"
		}
		if r, ok := strings.CutPrefix(s, local+string(filepath.Separator)); ok {
			return "/" + r
		}
		return s
	}
	req := GitRequest{Cwd: strip(dir)}
	for _, a := range args {
		req.Args = append(req.Args, strip(a))
	}
	var rep GitReply
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if err := peerPost(ctx, p, "/api/git", req, &rep); err != nil {
		return nil, true, err
	}
	if rep.Error != "" {
		return rep.Out, true, errors.New(rep.Error)
	}
	if len(args) > 0 && args[0] == "rev-parse" && slices.Contains(args, "--show-toplevel") {
		return []byte(filepath.Join(local, strings.TrimSpace(string(rep.Out))) + "\n"), true, nil
	}
	return rep.Out, true, nil
}

func peerPost(ctx context.Context, p Peer, path string, body, v any) error {
	b, err := jsonx.Marshal(body)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, p.URL+path, bytes.NewReader(b))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+p.Token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, gitOutMost*2))
	if err != nil {
		return err
	}
	if resp.StatusCode != http.StatusOK {
		var e struct{ Error string }
		_ = jsonx.Unmarshal(raw, &e)
		return fmt.Errorf("%s: %s", p.Name, or(e.Error, resp.Status))
	}
	return jsonx.Unmarshal(raw, v)
}
