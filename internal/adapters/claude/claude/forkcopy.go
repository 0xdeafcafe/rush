package claude

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/0xdeafcafe/photon/uithread"
)

// CopyTranscript writes the first upTo bytes of the conversation at src (all
// of it when upTo < 0) to dst as a conversation of its own, sessionID,
// ready for claude --resume. The original is left as it is.
func CopyTranscript(src, dst, oldID, sessionID string, upTo int64) error {
	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()
	if err := os.MkdirAll(filepath.Dir(dst), 0o700); err != nil {
		return err
	}
	if _, err := os.Stat(dst); err == nil {
		return fmt.Errorf("%s already exists", dst)
	}
	out, err := os.OpenFile(dst, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	var rd io.Reader = in
	if upTo >= 0 {
		rd = io.LimitReader(in, upTo)
	}
	r := bufio.NewReaderSize(rd, 256<<10)
	w := bufio.NewWriterSize(out, 256<<10)
	from := []byte(`"sessionId":"` + oldID + `"`)
	to := []byte(`"sessionId":"` + sessionID + `"`)
	for {
		line, rerr := r.ReadBytes('\n')
		if len(line) > 0 {
			if oldID != "" {
				line = bytes.ReplaceAll(line, from, to)
			}
			if _, err := w.Write(line); err != nil {
				out.Close()
				os.Remove(dst)
				return err
			}
		}
		if rerr == io.EOF {
			break
		}
		if rerr != nil {
			out.Close()
			os.Remove(dst)
			return rerr
		}
	}
	if err := w.Flush(); err != nil {
		out.Close()
		os.Remove(dst)
		return err
	}
	return out.Close()
}

// TranscriptPath is where a session's conversation lives for an account
// and folder.
func (a Account) TranscriptPath(cwd, sessionID string) string {
	return filepath.Join(a.ProjectsDir(), ProjectSlug(cwd), sessionID+".jsonl")
}

// FindTranscript is where a session's conversation is now. Entering or
// leaving a worktree moves it to that folder's project, so when it isn't
// under the folder the session started in, it's looked for under every
// project; TranscriptPath when it's nowhere yet. It reads the disk, so
// never on the UI.
func (a Account) FindTranscript(cwd, sessionID string) string {
	uithread.Forbid("claude.FindTranscript")
	p := a.TranscriptPath(cwd, sessionID)
	if _, err := os.Stat(p); err == nil {
		return p
	}
	if ms, _ := filepath.Glob(filepath.Join(a.ProjectsDir(), "*", sessionID+".jsonl")); len(ms) > 0 {
		return ms[0]
	}
	return p
}

// CopyCheckpoints gives conversation to the file checkpoints (the files'
// old contents) that conversation from kept, so a copy of it can still put
// files back as they were. Claude Code only brings them over itself when
// the transcript still names the old session, which a copy doesn't. The
// backups never change once written, so they're linked, not copied.
func (a Account) CopyCheckpoints(from, to string) error {
	src := filepath.Join(a.ConfigDir, "file-history", from)
	names, err := os.ReadDir(src)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	dst := filepath.Join(a.ConfigDir, "file-history", to)
	if err := os.MkdirAll(dst, 0o700); err != nil {
		return err
	}
	for _, n := range names {
		if n.IsDir() {
			continue
		}
		s, d := filepath.Join(src, n.Name()), filepath.Join(dst, n.Name())
		if _, err := os.Lstat(d); err == nil {
			continue
		}
		if os.Link(s, d) == nil {
			continue
		}
		b, err := os.ReadFile(s)
		if err != nil {
			return err
		}
		if err := os.WriteFile(d, b, 0o600); err != nil {
			return err
		}
	}
	return nil
}
