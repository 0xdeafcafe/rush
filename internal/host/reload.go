package host

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/selfexe"
	"github.com/0xdeafcafe/rush/internal/update"
)

// A reload restarts a host on the installed rush only once that rush has
// said it can run the session, and puts the session back on the rush
// before it when the new host doesn't come up.

// installedExe is the rush a restart starts hosts on: the one installed
// where this one runs from. Tests point it elsewhere.
var installedExe = selfexe.Path

// Preflight is whether this rush can run session id's host: its config
// reads, its folder is still there (a removed worktree isn't), its agent
// is one this rush knows and can drive, and the program it was told to
// run is there. It touches nothing of the session's; it
// is what `rush host preflight <id>` answers.
func Preflight(id string) error {
	cfg, err := ReadConfig(id)
	if err != nil {
		return err
	}
	a, ok := agent.Get(agent.Migrated(cfg.Kind))
	if !ok {
		return fmt.Errorf("rush doesn't know the agent %q", cfg.Kind)
	}
	if _, ok := a.(agent.Driver); !ok {
		return fmt.Errorf("rush can't run %s", a.Name())
	}
	if cfg.Cwd != "" {
		if _, err := os.Stat(cfg.Cwd); err != nil {
			return fmt.Errorf("its folder is gone: %w", err)
		}
	}
	if cfg.Binary != "" {
		if _, err := exec.LookPath(cfg.Binary); err != nil {
			return fmt.Errorf("%s's program: %w", a.Name(), err)
		}
	}
	return nil
}

// PreflightOK is the line `rush host preflight` prints when it can.
const PreflightOK = `{"ok":true}`

// preflight asks the rush at exe whether it can run session id's host.
// A rush from before it could be asked answers with its usage, and is let
// through as before.
func preflight(exe, id string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, exe, "host", "preflight", id).CombinedOutput()
	switch {
	case err == nil && bytes.Contains(out, []byte(PreflightOK)):
		return nil
	case bytes.Contains(out, []byte("usage: rush host run")):
		return nil
	case ctx.Err() != nil:
		return errors.New("it didn't answer in 5s")
	case err == nil:
		return errors.New("it didn't say ok: " + lastLine(string(out)))
	}
	if l := strings.TrimSpace(lastLine(string(out))); l != "" {
		return errors.New(l)
	}
	return err
}

// PrevExe is where the rush installed before the one at exe is kept
// (update.Install keeps it): the way back.
func PrevExe(exe string) string {
	if p, err := filepath.EvalSymlinks(exe); err == nil {
		exe = p
	}
	return update.Prev(exe)
}

// rollBack starts session cfg's host again on the rush installed before
// exe, whose host didn't come up (failed says how), and logs what that
// host last wrote. The restart still failed: the session is on the old
// rush, as before it.
func rollBack(cfg Config, exe string, failed error) error {
	prev := PrevExe(exe)
	f := map[string]any{"exe": exe, "prev": prev, "err": failed.Error(), "log": logTail(cfg.ID, 2<<10)}
	if _, err := spawn(cfg, prev); err != nil {
		f["prevErr"] = err.Error()
		logEvent(cfg.ID, "reload-failed", f)
		return fmt.Errorf("%w, and the rush before it didn't start it either: %v", failed, err)
	}
	logEvent(cfg.ID, "reload-rolled-back", f)
	return fmt.Errorf("%w, so it runs on the rush before it again", failed)
}

// logTail is the last n bytes of session id's host.log.
func logTail(id string, n int64) string {
	f, err := os.Open(filepath.Join(dir(id), "host.log"))
	if err != nil {
		return ""
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.Size() > n {
		_, _ = f.Seek(-n, io.SeekEnd)
	}
	b, _ := io.ReadAll(f)
	return string(b)
}
