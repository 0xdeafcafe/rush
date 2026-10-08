package claude

import (
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Job is one background session as Claude Code records it under
// jobs/<short>: the row rush shows, and what Claude Code needs to start
// it again.
type Job struct {
	agent.Job
	CLIVersion   string
	RespawnFlags []string
}

type Todo = agent.Todo

type Task = agent.Task

type jobFile struct {
	State          string         `json:"state"`
	Detail         string         `json:"detail"`
	Tempo          string         `json:"tempo"`
	Needs          jsontext.Value `json:"needs"`
	Intent         string         `json:"intent"`
	DisplayIntent  string         `json:"displayIntent"`
	Name           string         `json:"name"`
	NameSource     string         `json:"nameSource"`
	Cwd            string         `json:"cwd"`
	SessionID      string         `json:"sessionId"`
	LinkScanPath   string         `json:"linkScanPath"`
	CLIVersion     string         `json:"cliVersion"`
	RespawnFlags   []string       `json:"respawnFlags"`
	WorktreePath   string         `json:"worktreePath"`
	WorktreeBranch string         `json:"worktreeBranch"`
	Children       jsontext.Value `json:"children"`
	InFlightRaw    *struct {
		Tasks  int `json:"tasks"`
		Queued int `json:"queued"`
	} `json:"inFlight"`
	Fan []struct {
		Kind      string `json:"kind"`
		Label     string `json:"label"`
		StartedAt int64  `json:"startedAt"`
		DoneAt    int64  `json:"doneAt"`
	} `json:"fan"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
}

func LoadJob(a Account, id string) (Job, error) {
	p := filepath.Join(a.JobsDir(), id, "state.json")
	st, err := os.Stat(p)
	if err != nil {
		return Job{}, err
	}
	b, err := os.ReadFile(p)
	if err != nil {
		return Job{}, err
	}
	var f jobFile
	if err := jsonx.Unmarshal(b, &f); err != nil {
		return Job{}, err
	}
	j := Job{Job: agent.Job{
		ID: id, Account: a.Name, Name: f.Name, NameSource: f.NameSource,
		State: f.State, Detail: f.Detail, Tempo: f.Tempo, Intent: f.Intent,
		Cwd: f.Cwd, SessionID: f.SessionID, TranscriptPath: f.LinkScanPath,
		WorktreePath: f.WorktreePath, WorktreeBranch: f.WorktreeBranch,
		CreatedAt: f.CreatedAt, UpdatedAt: f.UpdatedAt, ModTime: st.ModTime(),
	}, CLIVersion: f.CLIVersion, RespawnFlags: f.RespawnFlags}
	if f.DisplayIntent != "" {
		j.Intent = f.DisplayIntent
	}
	if len(f.Needs) > 0 && f.Needs[0] == '"' {
		_ = jsonx.Unmarshal(f.Needs, &j.Needs)
	}
	if len(f.Children) > 0 && f.Children[0] == '[' {
		var c []jsontext.Value
		if jsonx.Unmarshal(f.Children, &c) == nil {
			j.Children = len(c)
		}
	}
	if f.InFlightRaw != nil {
		j.InFlight = f.InFlightRaw.Tasks + f.InFlightRaw.Queued
	}
	for _, x := range f.Fan {
		if x.Kind == "todo" {
			j.Todos++
			if x.DoneAt > 0 {
				j.TodosDone++
			}
			j.TodoItems = append(j.TodoItems, Todo{Label: x.Label, Done: x.DoneAt > 0, Started: x.StartedAt > 0})
			continue
		}
		if x.DoneAt == 0 && x.Label != "" {
			j.Running = append(j.Running, Task{Kind: x.Kind, Label: x.Label, StartedAt: time.UnixMilli(x.StartedAt)})
		}
		if x.Kind != "todo" && x.DoneAt == 0 && x.Label != "" {
			j.Background = append(j.Background, x.Kind+"\x00"+x.Label)
			if x.Kind == "agent" {
				j.Subagents++
			}
		}
	}
	if j.Name == "" {
		j.Name = id
	}
	if j.TranscriptPath == "" && j.SessionID != "" {
		j.TranscriptPath = filepath.Join(a.ProjectsDir(), ProjectSlug(j.Cwd), j.SessionID+".jsonl")
	}
	return j, nil
}

func ListJobIDs(a Account) []string {
	ents, err := os.ReadDir(a.JobsDir())
	if err != nil {
		return nil
	}
	ids := make([]string, 0, len(ents))
	for _, e := range ents {
		if e.IsDir() && len(e.Name()) == 8 {
			ids = append(ids, e.Name())
		}
	}
	sort.Strings(ids)
	return ids
}

// ProjectSlug is how Claude Code names a folder's project directory: every
// character but a letter or digit becomes a dash.
func ProjectSlug(dir string) string {
	b := []byte(realDir(dir))
	for i, c := range b {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
			b[i] = '-'
		}
	}
	return string(b)
}

// realDir is dir as Claude Code files it, by its real path: on a Mac
// /tmp, /var and /etc are links into /private.
// ponytail: only the system's own links, as it takes no disk reads; a
// folder reached through a link of the user's own still files elsewhere.
func realDir(dir string) string {
	if runtime.GOOS != "darwin" {
		return dir
	}
	for _, p := range []string{"/tmp", "/var", "/etc"} {
		if dir == p || strings.HasPrefix(dir, p+"/") {
			return "/private" + dir
		}
	}
	return dir
}

type TimelineEntry struct {
	At     time.Time `json:"at"`
	State  string    `json:"state"`
	Detail string    `json:"detail"`
}

func ReadTimeline(a Account, id string) []TimelineEntry {
	b, err := os.ReadFile(filepath.Join(a.JobsDir(), id, "timeline.jsonl"))
	if err != nil {
		return nil
	}
	var out []TimelineEntry
	for len(b) > 0 {
		i := indexByte(b, '\n')
		line := b
		if i >= 0 {
			line, b = b[:i], b[i+1:]
		} else {
			b = nil
		}
		var e TimelineEntry
		if jsonx.Unmarshal(line, &e) == nil {
			out = append(out, e)
		}
	}
	return out
}

func indexByte(b []byte, c byte) int {
	for i, x := range b {
		if x == c {
			return i
		}
	}
	return -1
}

func pinsPath(a Account) string { return filepath.Join(a.JobsDir(), "pins.json") }

// PinsPath is where Claude Code keeps an account's pins.
func PinsPath(a Account) string { return pinsPath(a) }

// ReadPins shares the native view's pin list so a pin shows in both views.
func ReadPins(a Account) []string {
	ids, _ := LoadPins(a)
	return ids
}

// LoadPins reports a pin list it could not read, so a write never replaces a
// list that is mid-write or in an unknown format.
func LoadPins(a Account) ([]string, error) {
	b, err := os.ReadFile(pinsPath(a))
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var ids []string
	if err := jsonx.Unmarshal(b, &ids); err != nil {
		return nil, fmt.Errorf("couldn't read the pin list: %w", err)
	}
	return ids, nil
}

func WritePins(a Account, ids []string) error {
	b, err := jsonx.MarshalIndent(ids)
	if err != nil {
		return err
	}
	return writeAtomic(pinsPath(a), b)
}

func writeAtomic(path string, b []byte) error {
	tmp := path + ".rush.tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
