package plugind

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

// Limits on the programs a plugin runs.
const (
	execTimeout = time.Minute
	maxExecArgs = 64
	maxExecArg  = 4 << 10
	maxExecIn   = 1 << 20
	maxExecOut  = 1 << 20 // each of stdout and stderr
	maxExecs    = 4       // running at once, per plugin
)

// exec runs a program the manifest names, outside the sandbox and as you,
// with the arguments the plugin adds to its fixed command line: in cwd if
// that's in one of its workspaces, else its data folder.
func (r *runner) exec(ctx context.Context, p plugin.Plugin, name string, args []string, stdin, cwd string) (any, error) {
	bad := func(msg string) error { return &plugin.Error{Code: plugin.CodeInvalidParams, Message: msg} }
	argv := p.ExecArgv(name)
	if argv == nil {
		return nil, plugin.Denied("exec " + name + ": its manifest names no such program")
	}
	if len(args) > maxExecArgs || len(stdin) > maxExecIn {
		return nil, bad("too many arguments or too much input")
	}
	for _, a := range args {
		if len(a) > maxExecArg || strings.ContainsRune(a, 0) {
			return nil, bad("an argument is too long or has a NUL in it")
		}
	}
	dir := plugin.DataDir(p.Name)
	if cwd == "" {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, err
		}
	} else {
		d, err := filepath.EvalSymlinks(cwd)
		if err != nil || !filepath.IsAbs(cwd) {
			return nil, bad("cwd does not exist")
		}
		if !slices.ContainsFunc(p.WorkspaceDirs(), func(w string) bool { return within(d, w) }) {
			return nil, plugin.Denied(cwd + " is outside the plugin's workspaces")
		}
		dir = d
	}
	select {
	case r.execs <- struct{}{}:
		defer func() { <-r.execs }()
	default:
		return nil, plugin.Denied(fmt.Sprintf("it already has %d programs running", maxExecs))
	}

	ctx, cancel := context.WithTimeout(ctx, execTimeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, argv[0], append(argv[1:], args...)...)
	cmd.Dir = dir
	cmd.Env = os.Environ()
	cmd.Stdin = strings.NewReader(stdin)
	// Its output goes to files, not pipes: a node program that exits
	// right after writing leaves a pipe with only its first 64 KB.
	outF, err := os.CreateTemp("", "rush-exec-out-*")
	if err != nil {
		return nil, err
	}
	defer os.Remove(outF.Name())
	defer outF.Close()
	errF, err := os.CreateTemp("", "rush-exec-err-*")
	if err != nil {
		return nil, err
	}
	defer os.Remove(errF.Name())
	defer errF.Close()
	cmd.Stdout, cmd.Stderr = outF, errF
	cmd.WaitDelay = 2 * time.Second
	r.log.Printf("runs %s %s", name, strings.Join(args, " "))
	err = cmd.Run()
	var out, errOut capped
	for _, f := range []struct {
		f *os.File
		c *capped
	}{{outF, &out}, {errF, &errOut}} {
		if _, err := f.f.Seek(0, io.SeekStart); err == nil {
			_, _ = io.Copy(struct{ io.Writer }{f.c}, io.LimitReader(f.f, maxExecOut+1)) // through Write, which caps
		}
	}
	code := 0
	if err != nil {
		var ee *exec.ExitError
		switch {
		case ctx.Err() == context.DeadlineExceeded:
			return nil, errors.New(name + " took over " + execTimeout.String() + " and was stopped")
		case errors.As(err, &ee):
			code = ee.ExitCode()
		default:
			return nil, err
		}
	}
	return map[string]any{"code": code, "stdout": out.String(), "stderr": errOut.String(),
		"truncated": out.over || errOut.over}, nil
}

// capped keeps the first maxExecOut bytes written to it.
type capped struct {
	bytes.Buffer
	over bool
}

func (c *capped) Write(p []byte) (int, error) {
	if room := maxExecOut - c.Len(); len(p) > room {
		c.over = true
		c.Buffer.Write(p[:max(room, 0)])
		return len(p), nil
	}
	return c.Buffer.Write(p)
}
