package copilot

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/netwatch"
)

// A remote session is Copilot's coding agent working on GitHub: given an
// issue or a task, it works in a runner and opens a pull request. rush
// lists them and reads their logs; it doesn't start or steer them.

// remoteSession is one of Copilot's agent sessions, as its API gives it.
type remoteSession struct {
	ID            string `json:"id"`
	Name          string `json:"name"`
	State         string `json:"state"`
	RepoID        int64  `json:"repo_id"`
	ResourceType  string `json:"resource_type"`
	ResourceNum   int    `json:"resource_number"`
	ResourceState string `json:"resource_state"`
	Model         string `json:"model"`
	HeadRef       string `json:"head_ref"`
	Error         any    `json:"error"`
	CreatedAt     string `json:"created_at"`
	UpdatedAt     string `json:"last_updated_at"`
	CompletedAt   string `json:"completed_at"`
}

// done are the states a session ends in.
var done = map[string]bool{"completed": true, "cancelled": true, "failed": true, "timed_out": true, "error": true}

// listEvery is how often the sessions are asked for again: the list is
// read on every refresh of rush's.
const listEvery = 20 * time.Second

var (
	listMu    sync.Mutex
	listAt    time.Time
	listing   bool
	listed    []remoteSession
	listBase  string
	listRepos map[int64]string
)

// sessions are your latest remote sessions as last listed, with the API
// they came from and their repositories' names. It never waits on the
// network: rush lists agents on every refresh, the first before it draws
// anything, so a stale listing asks for a new one in the background and
// the next refresh has it.
func sessions() ([]remoteSession, string, map[int64]string) {
	listMu.Lock()
	defer listMu.Unlock()
	if !listing && time.Since(listAt) >= listEvery {
		listing, listAt = true, time.Now()
		go relist()
	}
	return listed, listBase, listRepos
}

// relist asks GitHub for your remote sessions, and names their
// repositories, while the network is up.
func relist() {
	var (
		base  string
		list  []remoteSession
		repos = map[int64]string{}
	)
	err := netwatch.Do("copilot sessions", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		var err error
		if base, err = apiBase(ctx); err != nil {
			return err
		}
		var r struct {
			Sessions []remoteSession `json:"sessions"`
		}
		if err := get(ctx, base+"/agents/sessions?page_number=1&page_size=50&sort=last_updated_at%2Cdesc", &r); err != nil {
			return err
		}
		list = r.Sessions
		for _, s := range list {
			if _, ok := repos[s.RepoID]; !ok {
				repos[s.RepoID] = repoName(ctx, s.RepoID)
			}
		}
		return nil
	})
	listMu.Lock()
	defer listMu.Unlock()
	listing = false
	if err == nil {
		listed, listBase, listRepos = list, base, repos
	}
}

// Live are your remote sessions still working.
func (a Adapter) Live(p agent.Profile) []agent.Session { return a.remote(p, false) }

// Past are your remote sessions that have ended.
func (a Adapter) Past(p agent.Profile) []agent.Session { return a.remote(p, true) }

func (Adapter) remote(p agent.Profile, ended bool) []agent.Session {
	list, base, repos := sessions()
	var out []agent.Session
	for _, r := range list {
		if done[r.State] != ended {
			continue
		}
		out = append(out, r.session(p, base, repos[r.RepoID]))
	}
	return out
}

// session is a remote session as rush's own.
func (r remoteSession) session(p agent.Profile, base, repo string) agent.Session {
	s := agent.Session{Kind: Kind, Profile: p, ID: r.ID, Name: r.Name, Model: strings.TrimPrefix(r.Model, "sweagent-capi:"),
		Remote: true, Repo: repo, Transcript: base + "/agents/sessions/" + r.ID + "/logs",
		CreatedAt: parseTime(r.CreatedAt), UpdatedAt: parseTime(r.UpdatedAt)}
	switch {
	case !done[r.State]:
		s.State = "working"
		s.Detail = "working on GitHub"
	case r.State == "completed":
		s.State = "done"
	default:
		s.State = "stopped"
		s.Detail = strings.ReplaceAll(r.State, "_", " ")
	}
	if r.ResourceType == "pull" && r.ResourceNum > 0 && s.Repo != "" {
		s.PRs = []agent.PR{{URL: fmt.Sprintf("https://github.com/%s/pull/%d", s.Repo, r.ResourceNum), Number: r.ResourceNum,
			Title: r.Name, State: strings.ToUpper(r.ResourceState)}}
	}
	return s
}

func parseTime(s string) time.Time {
	t, _ := time.Parse(time.RFC3339Nano, s)
	return t
}

// History reads a remote session's log as the events a session sends.
// The task it was given isn't in the log: its name stands for it.
func (Adapter) History(s agent.Session, before time.Time) ([]event.Event, error) {
	if !s.Remote && !strings.HasPrefix(s.Transcript, "https://") {
		return nil, fmt.Errorf("copilot: session %s has no log rush can read", s.ID)
	}
	url := s.Transcript
	if url == "" {
		base, err := apiBase(context.Background())
		if err != nil {
			return nil, err
		}
		url = base + "/agents/sessions/" + s.ID + "/logs"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	body, err := fetch(ctx, url)
	if err != nil {
		return nil, err
	}
	defer body.Close()
	evs := []event.Event{event.Message{Role: "user", Parts: []event.Part{{Kind: event.Text, Text: firstNonEmpty(s.Name, "a task on GitHub")}}}}
	evs = append(evs, readLog(body, before)...)
	if s.State == "done" || s.State == "stopped" {
		evs = append(evs, event.TurnEnd{Reason: "done"})
	}
	return evs, nil
}

// chunk is one line of a session's log: a chat completion chunk, whose
// tool calls come twice, as made and then with what they returned.
type chunk struct {
	ID      string         `json:"id"`
	Created jsontext.Value `json:"created"`
	Choices []struct {
		Finish string `json:"finish_reason"`
		Delta  struct {
			Content   string `json:"content"`
			ToolCalls []struct {
				ID       string `json:"id"`
				Function struct {
					Name      string `json:"name"`
					Arguments string `json:"arguments"`
				} `json:"function"`
			} `json:"tool_calls"`
		} `json:"delta"`
	} `json:"choices"`
}

// readLog turns a session's log into events, up to before when it isn't
// zero.
func readLog(r io.Reader, before time.Time) []event.Event {
	var out []event.Event
	seen := map[string]bool{}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), 64<<20)
	for sc.Scan() {
		line, ok := strings.CutPrefix(sc.Text(), "data: ")
		if !ok {
			continue
		}
		var c chunk
		if jsonx.Unmarshal([]byte(line), &c) != nil {
			continue
		}
		if at := created(c.Created); !before.IsZero() && !at.IsZero() && !at.Before(before) {
			break
		}
		for _, ch := range c.Choices {
			d := ch.Delta
			if len(d.ToolCalls) == 0 {
				if strings.TrimSpace(d.Content) != "" {
					out = append(out, event.Message{Role: "assistant", ID: c.ID, Parts: []event.Part{{Kind: event.Text, Text: d.Content}}})
				}
				continue
			}
			// A chunk with calls carries what they returned, when it
			// carries words at all: what the agent says comes on its own.
			for _, tc := range d.ToolCalls {
				if !seen[tc.ID] {
					seen[tc.ID] = true
					call := callOf(tc.ID, tc.Function.Name, tc.Function.Arguments)
					out = append(out, event.Message{Role: "assistant", ID: c.ID, Parts: []event.Part{{Kind: event.ToolCall, Call: &call}}})
				}
				if d.Content != "" {
					out = append(out, event.Message{Role: "user", Parts: []event.Part{{Kind: event.ToolResult,
						Output: &tool.Output{CallID: tc.ID, Text: d.Content}}}})
				}
			}
		}
	}
	return out
}

// created is a chunk's time: seconds or milliseconds, as a number or a
// string.
func created(raw jsontext.Value) time.Time {
	var n int64
	if jsonx.Unmarshal(raw, &n) != nil {
		var s string
		if jsonx.Unmarshal(raw, &s) != nil {
			return time.Time{}
		}
		fmt.Sscan(s, &n)
	}
	switch {
	case n > 1e12:
		return time.UnixMilli(n)
	case n > 0:
		return time.Unix(n, 0)
	}
	return time.Time{}
}

// callOf is one of the coding agent's tool calls as rush's own.
func callOf(id, name, arguments string) tool.Call {
	c := tool.Call{ID: id, Name: name, Raw: jsontext.Value(arguments)}
	var in struct {
		Command     string `json:"command"`
		Description string `json:"description"`
		Path        string `json:"path"`
		Paths       string `json:"paths"`
		Pattern     string `json:"pattern"`
		Query       string `json:"query"`
		OldStr      string `json:"old_str"`
		NewStr      string `json:"new_str"`
		FileText    string `json:"file_text"`
		Name        string `json:"name"`
	}
	_ = jsonx.Unmarshal([]byte(arguments), &in)
	in.Path, in.Paths = inRepo(in.Path), inRepo(in.Paths)
	in.Command = strings.ReplaceAll(in.Command, "cd "+runnerDir(in.Command)+" && ", "")
	x := &c.Input
	x.Description = in.Description
	switch name {
	case "bash":
		c.Kind, x.Command = tool.Shell, in.Command
	case "view":
		c.Kind, x.Path = tool.Read, in.Path
	case "create":
		c.Kind, x.Path, x.Content = tool.Write, in.Path, in.FileText
	case "edit", "str_replace", "str_replace_editor":
		c.Kind, x.Path = tool.Edit, in.Path
		x.Edits = []tool.Replace{{Old: in.OldStr, New: in.NewStr}}
	case "grep":
		c.Kind, x.Pattern, x.Path = tool.Search, in.Pattern, in.Paths
	case "glob":
		c.Kind, x.Pattern, x.Path = tool.Glob, in.Pattern, in.Paths
	case "think":
		c.Kind = tool.Think
	case "search_code_subagent":
		c.Kind, x.Pattern = tool.Search, in.Query
	case "run_setup", "run_custom_setup_step":
		c.Title = in.Name
	default:
		if server, t, ok := strings.Cut(name, "-server-"); ok {
			c.Kind, x.Server, x.Tool = tool.MCP, server, t
		}
	}
	return c
}

func firstNonEmpty(s ...string) string {
	for _, v := range s {
		if v != "" {
			return v
		}
	}
	return ""
}

// runnerRe is the checkout a runner works in: /home/runner/work/<repo>/<repo>/.
var runnerRe = regexp.MustCompile(`^/home/runner/work/[^/]+/[^/]+/?`)

// inRepo is a path in the runner's checkout, as a path in the repository.
func inRepo(p string) string {
	if r := runnerRe.ReplaceAllString(p, ""); r != "" {
		return r
	}
	return p
}

// cdRunnerRe is a command changing into the runner's checkout first.
var cdRunnerRe = regexp.MustCompile(`^cd (/home/runner/work/[^/ ]+/[^/ ]+) && `)

// runnerDir is the checkout a command changes into first, if it does.
func runnerDir(cmd string) string {
	if m := cdRunnerRe.FindStringSubmatch(cmd); m != nil {
		return m[1]
	}
	return "\x00"
}
