// Package memory is rush's side of an agent's memory notes: writing one
// from a few words, and which notes are worth a look.
package memory

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

const draftSystem = `You turn what a user wants an AI coding agent to remember into one memory note.
Answer with JSON only, no fence: {"name","title","description","type","body"}.
- name: short-kebab-case, 2-5 words.
- title: a few words, for the index.
- description: one line under 120 characters saying what the note is about, so it can be picked when relevant.
- type: "user" (who the user is, their preferences), "feedback" (how the agent should work: a correction or an approach to keep), "project" (ongoing work, goals or constraints of this project), or "reference" (where to find something outside the code).
- body: the fact in plain words. For feedback and project, follow it with a "**Why:**" line and a "**How to apply:**" line when the text gives a reason; never invent one.
Keep the user's own words and terms; don't add anything they didn't say. Convert relative dates to absolute ones using today's date.`

// Asker asks a cheap model; a variable so tests don't spend a call.
var Asker = func(ctx context.Context, system, prompt string) (string, error) {
	_, out, err := agent.AskCheap(ctx, system, prompt, "haiku")
	return out, err
}

// Draft turns what to remember, and what was said around it if anything,
// into a note, with a cheap model.
func Draft(ctx context.Context, what, around string) (agent.MemoryNote, error) {
	what = strings.TrimSpace(what)
	if what == "" && strings.TrimSpace(around) == "" {
		return agent.MemoryNote{}, errors.New("nothing to remember")
	}
	prompt := "Today is " + time.Now().Format("2006-01-02") + ".\n\n"
	if around != "" {
		prompt += "The conversation it comes from:\n<conversation>\n" + clip(around, 12_000) + "\n</conversation>\n\n"
	}
	if what != "" {
		prompt += "What to remember:\n" + what
	} else {
		prompt += "Remember the one thing from that conversation most worth knowing in future sessions."
	}
	out, err := Asker(ctx, draftSystem, prompt)
	if err != nil {
		return agent.MemoryNote{}, err
	}
	n, err := parseDraft(out)
	if err != nil {
		return agent.MemoryNote{}, fmt.Errorf("the model's note didn't read: %w", err)
	}
	return n, nil
}

func parseDraft(out string) (agent.MemoryNote, error) {
	var n agent.MemoryNote
	i := strings.Index(out, "{")
	if i < 0 {
		return n, errors.New("no JSON in it")
	}
	// The first object, whatever the model said around it.
	if err := json.NewDecoder(strings.NewReader(out[i:])).Decode(&n); err != nil {
		return n, err
	}
	if !ValidType(n.Type) {
		n.Type = "project"
	}
	if strings.TrimSpace(n.Body) == "" || strings.TrimSpace(n.Name+n.Title) == "" {
		return n, errors.New("no name or text")
	}
	return n, nil
}

// Types are the kinds of memory note.
var Types = []string{"user", "feedback", "project", "reference"}

// ValidType is whether t is one of Types.
func ValidType(t string) bool { return slices.Contains(Types, t) }

func clip(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return "…" + s[len(s)-n:]
}

// Settled is how long a project or reference note goes unchanged before
// it's worth asking whether it's still true.
const Settled = 60 * 24 * time.Hour

// Why is why a memory note is worth a look, "" when it isn't: what's
// untidy about it, or that it's about work or a place that may have moved
// on and hasn't changed in a while. A note kept as it is isn't, until it
// changes.
func Why(f agent.MemoryFile, now time.Time) string {
	if f.Group != "Auto memory" || f.Name == "MEMORY.md" || f.Missing || IsKept(f.Path, f.Mod) {
		return ""
	}
	if f.Warn != "" {
		return f.Warn
	}
	if (f.Kind == "project" || f.Kind == "reference") && !f.Missing && !f.Mod.IsZero() && now.Sub(f.Mod) > Settled {
		return "unchanged since " + f.Mod.Format("2 Jan") + " · is it still true?"
	}
	return ""
}

// keptPath is the file of notes looked at and kept as they are: path →
// when they were last changed then.
func keptPath() string { return filepath.Join(state.Dir(), "memory-kept.json") }

// keptMu keeps this process's reads and writes of it apart; another
// rush's write lands whole, by a rename.
var keptMu sync.Mutex

func readKept() map[string]int64 {
	m := map[string]int64{}
	if b, err := os.ReadFile(keptPath()); err == nil {
		_ = json.Unmarshal(b, &m)
	}
	return m
}

// IsKept is whether the note at path was kept as it is, and hasn't
// changed since. It reads the disk: not for the UI goroutine.
func IsKept(path string, mod time.Time) bool {
	keptMu.Lock()
	defer keptMu.Unlock()
	at, ok := readKept()[path]
	return ok && at == mod.Unix()
}

// Keep marks the note at path, as it is now, as looked at and kept.
func Keep(path string) error {
	st, err := os.Stat(path)
	if err != nil {
		return err
	}
	keptMu.Lock()
	defer keptMu.Unlock()
	m := readKept() // what another rush kept meanwhile stays
	m[path] = st.ModTime().Unix()
	for p := range m {
		if _, err := os.Stat(p); err != nil {
			delete(m, p)
		}
	}
	b, _ := json.Marshal(m)
	if err := os.MkdirAll(filepath.Dir(keptPath()), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(keptPath()), ".memory-kept-*")
	if err != nil {
		return err
	}
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return err
	}
	tmp.Close()
	return os.Rename(tmp.Name(), keptPath())
}
