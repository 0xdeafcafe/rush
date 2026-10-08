package claude

import (
	"bytes"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// FindSpawn looks for a Claude Code transcript begun after start, in one
// of profiles: in the folder of the project dir first, then in any
// project folder written to since, taking the first fits takes.
func (Adapter) FindSpawn(profiles []agent.Profile, dir string, start time.Time, fits func(agent.Session) bool) (agent.Session, bool) { //nolint:gocognit // two folder walks, moved whole from the UI
	seen := map[string]bool{}
	for _, p := range profiles {
		acct := claude.AccountOf(p)
		root := acct.ProjectsDir()
		if seen[root] {
			continue
		}
		seen[root] = true
		// Claude Code names the folder for where it really is: /tmp is
		// /private/tmp.
		dirs := []string{filepath.Join(root, claude.ProjectSlug(dir))}
		if real, err := filepath.EvalSymlinks(dir); err == nil && real != dir {
			dirs = append(dirs, filepath.Join(root, claude.ProjectSlug(real)))
		}
		ents, _ := os.ReadDir(root)
		for _, e := range ents {
			if fi, err := e.Info(); err == nil && e.IsDir() && !fi.ModTime().Before(start.Add(-3*time.Second)) {
				dirs = append(dirs, filepath.Join(root, e.Name()))
			}
		}
		for _, dir := range dirs {
			files, _ := os.ReadDir(dir)
			for _, f := range files {
				if f.IsDir() || !strings.HasSuffix(f.Name(), ".jsonl") {
					continue
				}
				fi, err := f.Info()
				if err != nil || fi.ModTime().Before(start) {
					continue
				}
				path := filepath.Join(dir, f.Name())
				if seen[path] {
					continue
				}
				seen[path] = true
				text, at := firstPrompt(path)
				s := agent.Session{Kind: Kind, Profile: p, ID: strings.TrimSuffix(f.Name(), ".jsonl"), Name: text, Transcript: path, CreatedAt: at}
				if !at.IsZero() && fits(s) {
					return s, true
				}
			}
		}
	}
	return agent.Session{}, false
}

// firstPrompt is the first thing a Claude Code transcript was asked, and
// when, read from its head alone.
func firstPrompt(path string) (string, time.Time) {
	f, err := os.Open(path)
	if err != nil {
		return "", time.Time{}
	}
	defer f.Close()
	b := make([]byte, 64<<10)
	n, _ := f.Read(b)
	for l := range bytes.SplitSeq(b[:n], []byte{'\n'}) {
		if !bytes.Contains(l, []byte(`"type":"user"`)) {
			continue
		}
		var line struct {
			Type        string    `json:"type"`
			IsSidechain bool      `json:"isSidechain"`
			Timestamp   time.Time `json:"timestamp"`
			Message     struct {
				Content jsontext.Value `json:"content"`
			} `json:"message"`
		}
		if jsonx.Unmarshal(l, &line) != nil || line.Type != "user" || line.IsSidechain {
			continue
		}
		var text string
		if jsonx.Unmarshal(line.Message.Content, &text) != nil {
			var blocks []struct{ Type, Text string }
			_ = jsonx.Unmarshal(line.Message.Content, &blocks)
			var sb strings.Builder
			for _, bl := range blocks {
				if bl.Type == "text" {
					sb.WriteString(bl.Text)
				}
			}
			text = sb.String()
		}
		return text, line.Timestamp
	}
	return "", time.Time{}
}

var _ agent.SpawnFinder = Adapter{}
