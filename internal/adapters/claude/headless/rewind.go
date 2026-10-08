package headless

import (
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// CheckpointEnv turns on Claude Code's file checkpoints for a headless
// session: before a turn edits a file, its old contents are kept, so the
// files can later be put back as they were before any of your messages.
const CheckpointEnv = "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING=1"

// FileRewind is what putting the files back did, or would do.
type FileRewind struct {
	CanRewind  bool     `json:"canRewind"`
	Error      string   `json:"error,omitempty"`
	Files      []string `json:"filesChanged,omitempty"`
	Insertions int      `json:"insertions,omitzero"`
	Deletions  int      `json:"deletions,omitzero"`
}

// RewindFiles puts the files a conversation changed back as they were just
// before the message userMessageID, the uuid of the transcript line that
// sent it. dryRun only says which files that would change. It runs a
// Claude Code of its own on the saved conversation (o.Resume), which sends
// nothing to the model and ends when it has answered.
func RewindFiles(o Options, userMessageID string, dryRun bool) (FileRewind, error) {
	if o.Resume == "" {
		return FileRewind{}, errors.New("no conversation to rewind")
	}
	o.Env = append(append([]string{}, o.Env...), CheckpointEnv)
	o.Tap, o.Skip = nil, nil
	s, err := Start(o)
	if err != nil {
		return FileRewind{}, err
	}
	defer s.Stop(5 * time.Second)
	id, err := s.control(map[string]any{"subtype": "rewind_files", "user_message_id": userMessageID, "dry_run": dryRun})
	if err != nil {
		return FileRewind{}, err
	}
	timeout := time.After(time.Minute)
	for {
		select {
		case ev, ok := <-s.Events:
			if !ok {
				return FileRewind{}, fmt.Errorf("Claude Code stopped before answering: %v", s.Err())
			}
			r, ok := ev.(ControlReply)
			if !ok || r.ID != id {
				continue
			}
			var out FileRewind
			if len(r.Body) > 0 {
				_ = jsonx.Unmarshal(r.Body, &out)
			}
			if r.Error != "" {
				return out, errors.New(r.Error)
			}
			return out, nil
		case <-timeout:
			return FileRewind{}, errors.New("Claude Code took too long to answer")
		}
	}
}

// Recap asks a throwaway copy of a saved conversation (o.Resume) what it
// learned since your message from: what was tried, what worked, what
// didn't. It's for carrying back to an earlier point that forgets those
// turns. Nothing is saved, and the conversation itself is left as it is.
func Recap(o Options, from string) (string, error) {
	if o.Resume == "" {
		return "", errors.New("no conversation to recap")
	}
	bin := o.Binary
	if bin == "" {
		bin = "claude"
	}
	if r := []rune(from); len(r) > 300 {
		from = string(r[:300]) + "…"
	}
	ask := "I'm about to rewind this conversation to just before my message:\n\n> " +
		strings.ReplaceAll(from, "\n", "\n> ") + "\n\n" +
		"Everything from that message on will be forgotten, and I'll try again from there. " +
		"Write the note I should bring back: what was tried, what worked, what didn't and why, " +
		"and any facts found (files, causes, commands) that would save time next time. " +
		"Say what code was changed. Under 200 words, plain bullets, no preamble. Don't use any tools."
	args := []string{"-p", "--resume", o.Resume, "--fork-session", "--no-session-persistence",
		"--max-turns", "1", "--output-format", "json"}
	if o.Model != "" {
		args = append(args, "--model", o.Model)
	}
	cmd := exec.Command(bin, append(args, ask)...)
	cmd.Dir = o.Dir
	cmd.Env = append(o.Account.Env(), o.Env...)
	var stderr tail
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	var r struct {
		Result  string `json:"result"`
		IsError bool   `json:"is_error"`
	}
	if jerr := jsonx.Unmarshal(out, &r); jerr != nil || r.IsError || strings.TrimSpace(r.Result) == "" {
		if err == nil {
			err = errors.New(firstNonEmpty(strings.TrimSpace(r.Result), "no answer"))
		}
		if msg := strings.TrimSpace(stderr.String()); msg != "" {
			err = fmt.Errorf("%w: %s", err, msg)
		}
		return "", fmt.Errorf("couldn't recap: %w", err)
	}
	return strings.TrimSpace(r.Result), nil
}

func firstNonEmpty(s ...string) string {
	for _, v := range s {
		if v != "" {
			return v
		}
	}
	return ""
}
