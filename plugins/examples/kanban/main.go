// Command kanban connects rush to kanban-code
// (github.com/langwatch/kanban-code), the board that shows coding agents as
// cards. kanban-code knows what the work is: cards, their issues, pull
// requests, checks and review threads. rush runs the agents. This plugin
// joins the two:
//
//   - Claude can read the board, and the card it's working on, with the
//     issue, the PR, failing checks and unresolved review threads.
//   - Claude (or you, through it) can start an agent on a card: in rush,
//     in the card's worktree or a new one, tagged with the card. Once the
//     agent's conversation has begun, the plugin has kanban-code link the
//     card to it, so the card follows the agent across the board.
//   - When a card's PR gets a failing check or a new review thread, the
//     agent working on it hears about it: the plugin queues it a message,
//     sent when its turn ends.
//   - rush's agent list can show the board: a section per column, and
//     each card's agent under the card's name (sidebar.set).
//
// It reads ~/.kanban-code/links.json, and changes cards only through the
// kanban CLI, which asks the running app to: the app owns the file.
//
//	mkdir -p ~/.config/rush/plugins/kanban
//	go build -o ~/.config/rush/plugins/kanban/kanban ./plugins/examples/kanban
//	cp plugins/examples/kanban/plugin.json ~/.config/rush/plugins/kanban/
//	rush plugin approve kanban
package main

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime/debug"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// session is what rush says about one of its sessions.
type session struct {
	ID        string            `json:"id"`
	SessionID string            `json:"sessionId"`
	Name      string            `json:"name"`
	Cwd       string            `json:"cwd"`
	Branch    string            `json:"branch"`
	State     string            `json:"state"`
	StartedBy string            `json:"startedBy"`
	Meta      map[string]string `json:"meta"`
}

func (s session) live() bool { return s.State != "stopped" }

var (
	conn *plugin.Conn
	data string // the plugin's data folder

	mu       sync.Mutex
	sessions = map[string]session{} // rush's sessions, by rush id
	// watching closes once rush has sent its sessions: until then, a card
	// can't tell it already has an agent.
	watching = make(chan struct{})
	st       memory
	linking  = map[string]bool{}      // relinks under way, by session
	tried    = map[string]time.Time{} // when each was last tried
)

// memory is what the plugin keeps across restarts, in its data folder.
type memory struct {
	// Linked is the card each conversation has been linked to, by Claude
	// Code session id; Tries counts failed attempts.
	Linked map[string]string `json:"linked"`
	Tries  map[string]int    `json:"tries"`
	// Seen is each card's PRs as last looked at, so only what's new is
	// passed on.
	Seen map[string]map[int]prSeen `json:"seen"`
}

type prSeen struct {
	Failing []string `json:"failing,omitempty"`
	Threads int      `json:"threads,omitzero"`
}

func main() {
	f := os.NewFile(3, "rush")
	if f == nil {
		fmt.Fprintln(os.Stderr, "run me from rush: I talk on fd 3")
		os.Exit(2)
	}
	// Well under the 64 MB the manifest asks for: the board is read often,
	// and garbage would otherwise pile up past it.
	debug.SetMemoryLimit(40 << 20)
	conn = plugin.NewConn(f, handle)
	<-conn.Done()
}

func handle(ctx context.Context, method string, params jsontext.Value) (any, error) {
	switch method {
	case "initialize":
		var in struct {
			DataDir string `json:"dataDir"`
		}
		_ = jsonx.Unmarshal(params, &in)
		data = in.DataDir
		load()
		go follow()
		go showBoard()
		return map[string]any{}, nil
	case "tools.list":
		return map[string]any{"tools": tools}, nil
	case "tools.call":
		var in struct {
			Session   string            `json:"session"`
			SessionID string            `json:"sessionId"`
			Cwd       string            `json:"cwd"`
			Meta      map[string]string `json:"meta"`
			Name      string            `json:"name"`
			Arguments jsontext.Value    `json:"arguments"`
		}
		if err := jsonx.Unmarshal(params, &in); err != nil {
			return nil, err
		}
		me := session{ID: in.Session, SessionID: in.SessionID, Cwd: in.Cwd, Meta: in.Meta}
		text, err := call(ctx, me, in.Name, in.Arguments)
		if err != nil {
			return result(err.Error(), true), nil
		}
		return result(text, false), nil
	case "session.changed":
		var s session
		if jsonx.Unmarshal(params, &s) == nil {
			mu.Lock()
			sessions[s.ID] = s
			mu.Unlock()
			go link(s)
		}
		return nil, nil
	case "session.gone":
		var s struct{ ID string }
		_ = jsonx.Unmarshal(params, &s)
		mu.Lock()
		delete(sessions, s.ID)
		mu.Unlock()
		return nil, nil
	}
	return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
}

// follow watches rush's sessions, and every minute looks at the board for
// news for the agents working on cards.
func follow() {
	// rush takes the watch once initialize has been answered.
	for {
		var list []session
		err := conn.Call(context.Background(), "sessions.watch", nil, &list)
		if err == nil {
			mu.Lock()
			for _, s := range list {
				sessions[s.ID] = s
			}
			mu.Unlock()
			close(watching)
			for _, s := range list {
				go link(s)
			}
			break
		}
		select {
		case <-conn.Done():
			return
		case <-time.After(time.Second):
		}
	}
	t := time.NewTicker(time.Minute)
	defer t.Stop()
	for {
		review()
		select {
		case <-conn.Done():
			return
		case <-t.C:
		}
		mu.Lock()
		all := make([]session, 0, len(sessions))
		for _, s := range sessions {
			all = append(all, s)
		}
		mu.Unlock()
		for _, s := range all {
			link(s) // retries what failed, kanban-code not running say
		}
	}
}

// link has kanban-code link a card to the conversation of the agent the
// plugin started for it, once there is one. Only the app may change a
// card, so the kanban CLI asks it to; if it isn't running, this tries again
// later.
func link(s session) {
	cardID := s.Meta["card"]
	if s.StartedBy != "kanban" || cardID == "" || s.SessionID == "" {
		return
	}
	mu.Lock()
	if st.Linked[s.SessionID] == cardID || linking[s.SessionID] || st.Tries[s.SessionID] >= 20 ||
		time.Since(tried[s.SessionID]) < 30*time.Second {
		mu.Unlock()
		return
	}
	linking[s.SessionID] = true
	tried[s.SessionID] = time.Now()
	mu.Unlock()
	defer func() {
		mu.Lock()
		delete(linking, s.SessionID)
		mu.Unlock()
	}()
	out, err := kanban(context.Background(), "relink", cardID, s.SessionID, "--json")
	mu.Lock()
	if err != nil {
		st.Tries[s.SessionID]++
		logf("linking card %s to %s: %v (try %d)", cardID, s.SessionID, err, st.Tries[s.SessionID])
	} else {
		st.Linked[s.SessionID] = cardID
		delete(st.Tries, s.SessionID)
		logf("linked card %s to %s: %s", cardID, s.SessionID, strings.TrimSpace(out))
	}
	save()
	mu.Unlock()
}

// review passes on what's new on the PRs of cards an agent in rush is
// working on: a check that started failing, review threads that appeared.
// The first look at a card only notes how it is.
func review() {
	cards, err := readCards()
	if err != nil {
		return
	}
	mu.Lock()
	var live []session
	for _, s := range sessions {
		if s.live() {
			live = append(live, s)
		}
	}
	mu.Unlock()
	for _, s := range live {
		c, ok := cardOf(cards, s)
		if !ok || len(c.PRLinks) == 0 {
			continue
		}
		mu.Lock()
		was, known := st.Seen[c.ID]
		now, news := prNews(c.PRLinks, was, known)
		st.Seen[c.ID] = now
		save()
		mu.Unlock()
		if len(news) == 0 {
			continue
		}
		text := "News on your kanban card, " + c.title() + ":\n- " + strings.Join(news, "\n- ") +
			"\nLook into it (gh pr checks, gh pr view --comments), fix what's yours, push, and say what you did."
		if err := conn.Call(context.Background(), "sessions.queue", map[string]any{"id": s.ID, "text": text}, nil); err != nil {
			logf("telling %s about card %s: %v", s.ID, c.ID, err)
		}
	}
}

// prNews compares a card's PRs with how they were last seen: checks that
// started failing, and review threads that appeared. Not known yet, there's
// nothing to compare with, so no news.
func prNews(prs []pr, was map[int]prSeen, known bool) (map[int]prSeen, []string) {
	now := map[int]prSeen{}
	var news []string
	for _, p := range prs {
		seen := prSeen{Failing: p.failing(), Threads: p.UnresolvedThreads}
		now[p.Number] = seen
		if !known {
			continue
		}
		old := was[p.Number]
		var newly []string
		for _, f := range seen.Failing {
			if !slices.Contains(old.Failing, f) {
				newly = append(newly, f)
			}
		}
		if len(newly) > 0 {
			news = append(news, fmt.Sprintf("PR #%d: checks failing: %s.", p.Number, strings.Join(newly, ", ")))
		}
		if seen.Threads > old.Threads {
			news = append(news, fmt.Sprintf("PR #%d: %d unresolved review thread(s), %d new; the first is %s",
				p.Number, seen.Threads, seen.Threads-old.Threads, p.FirstUnresolvedThreadURL))
		}
	}
	return now, news
}

func call(ctx context.Context, me session, name string, args jsontext.Value) (string, error) {
	var in struct {
		Card   string `json:"card"`
		Column string `json:"column"`
		Prompt string `json:"prompt"`
		Mode   string `json:"permissionMode"`
		Text   string `json:"text"`
	}
	_ = jsonx.Unmarshal(args, &in)
	cards, err := readCards()
	if err != nil {
		return "", err
	}
	switch name {
	case "board":
		return board(cards, in.Column), nil

	case "my_card":
		c, ok := cardOf(cards, me)
		if !ok {
			return "", errors.New("this session isn't working on a card on the board")
		}
		return describe(c), nil

	case "start_card":
		c, err := findCard(cards, in.Card)
		if err != nil {
			return "", err
		}
		if s, ok := working(cards, c); ok {
			return "", fmt.Errorf("%s is already being worked on by %s (%s); send it a message with card_message", c.title(), s.Name, s.ID)
		}
		return start(ctx, c, in.Prompt, in.Mode)

	case "card_message":
		c, err := findCard(cards, in.Card)
		if err != nil {
			return "", err
		}
		s, ok := working(cards, c)
		if !ok {
			return "", fmt.Errorf("no agent in rush is working on %s; start one with start_card", c.title())
		}
		if err := conn.Call(ctx, "sessions.queue", map[string]any{"id": s.ID, "text": in.Text}, nil); err != nil {
			return "", err
		}
		return "Queued for " + s.Name + "; it goes when its turn ends.", nil
	}
	return "", fmt.Errorf("no tool named %s", name)
}

// working is the live rush session working on a card, if there is one.
func working(cards []card, c card) (session, bool) {
	// Just started, rush may not have said yet which agents are running:
	// wait a moment rather than start a second one on the card.
	select {
	case <-watching:
	case <-time.After(3 * time.Second):
	}
	mu.Lock()
	defer mu.Unlock()
	for _, s := range sessions {
		if o, ok := cardOf(cards, s); ok && o.ID == c.ID && s.live() {
			return s, true
		}
	}
	return session{}, false
}

// start starts an agent on a card: in its worktree if it has one, else in
// a new worktree of its project, on a branch named after it.
func start(ctx context.Context, c card, prompt, mode string) (string, error) {
	if prompt = strings.TrimSpace(prompt); prompt == "" {
		prompt = task(c)
	}
	req := map[string]any{"prompt": prompt, "name": c.title(), "meta": map[string]string{"card": c.ID}}
	if mode != "" {
		req["permissionMode"] = mode
	}
	if wt := c.WorktreeLink; wt != nil && exists(wt.Path) {
		req["cwd"] = wt.Path
	} else if c.ProjectPath != "" && exists(c.ProjectPath) {
		s := slug(c.title())
		req["cwd"] = c.ProjectPath
		req["worktree"] = map[string]any{"name": s, "branch": s + "-" + c.ID[max(len(c.ID)-6, 0):]}
	} else {
		return "", fmt.Errorf("%s has no project folder or worktree to work in", c.title())
	}
	var out struct {
		ID  string `json:"id"`
		Cwd string `json:"cwd"`
	}
	if err := conn.Call(ctx, "sessions.start", req, &out); err != nil {
		return "", err
	}
	return fmt.Sprintf("Started an agent on %s: %s, in %s. kanban-code links the card to it once its conversation has begun.",
		c.title(), out.ID, out.Cwd), nil
}

// task is what an agent starting on a card is asked to do.
func task(c card) string {
	var b strings.Builder
	switch {
	case c.PromptBody != "":
		b.WriteString(c.PromptBody)
	case c.IssueLink != nil:
		fmt.Fprintf(&b, "Resolve issue #%d: %s\n%s", c.IssueLink.Number, c.IssueLink.Title, c.IssueLink.URL)
		if body := strings.TrimSpace(c.IssueLink.Body); body != "" {
			b.WriteString("\n\n" + body)
		}
	default:
		b.WriteString(c.title())
	}
	b.WriteString("\n\nThis is kanban card " + c.ID + "; mcp__rush-kanban__my_card shows it, with its PR and review state once there is one.")
	return b.String()
}

// kanban runs the kanban CLI, which rush runs for the plugin, outside its
// sandbox.
func kanban(ctx context.Context, args ...string) (string, error) {
	var out struct {
		Code   int    `json:"code"`
		Stdout string `json:"stdout"`
		Stderr string `json:"stderr"`
	}
	if err := conn.Call(ctx, "exec", map[string]any{"name": "kanban", "args": args}, &out); err != nil {
		return "", err
	}
	if out.Code != 0 {
		return "", errors.New(strings.TrimSpace(or(out.Stderr, out.Stdout)))
	}
	return out.Stdout, nil
}

func exists(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

func load() {
	st = memory{Linked: map[string]string{}, Tries: map[string]int{}, Seen: map[string]map[int]prSeen{}}
	if b, err := os.ReadFile(filepath.Join(data, "state.json")); err == nil {
		_ = jsonx.Unmarshal(b, &st)
	}
	if st.Linked == nil {
		st.Linked = map[string]string{}
	}
	if st.Tries == nil {
		st.Tries = map[string]int{}
	}
	if st.Seen == nil {
		st.Seen = map[string]map[int]prSeen{}
	}
}

// save keeps the memory. Called with mu held.
func save() {
	b, _ := jsonx.MarshalIndent(st)
	tmp := filepath.Join(data, "state.json.tmp")
	if os.WriteFile(tmp, b, 0o600) == nil {
		_ = os.Rename(tmp, filepath.Join(data, "state.json"))
	}
}

func logf(f string, a ...any) {
	_ = conn.Notify("log", map[string]any{"message": fmt.Sprintf(f, a...)})
}

func result(text string, isErr bool) map[string]any {
	return map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isErr}
}

func schema(props map[string]string, required ...string) map[string]any {
	p := map[string]any{}
	for k, d := range props {
		p[k] = map[string]any{"type": "string", "description": d}
	}
	return map[string]any{"type": "object", "properties": p, "required": required, "additionalProperties": false}
}

var tools = []map[string]any{
	{"name": "board", "description": "The kanban-code board: every card by column (In Progress, Waiting, In Review, Backlog, Done), with its id, branch and PR status.",
		"inputSchema": schema(map[string]string{"column": "Only this column: in_progress, requires_attention, in_review, backlog or done."})},
	{"name": "my_card", "description": "The kanban-code card you're working on: its task or issue, its worktree, and its PRs with their status, failing checks and unresolved review threads.",
		"inputSchema": schema(map[string]string{})},
	{"name": "start_card", "description": "Start an agent in rush on a kanban-code card: in the card's worktree, or a new worktree of its project. It's asked to do the card's task or issue, unless you give a prompt. The card follows it across the board.",
		"inputSchema": schema(map[string]string{
			"card":           "The card's id, id prefix, or name.",
			"prompt":         "What to ask the agent, instead of the card's own task.",
			"permissionMode": "default, acceptEdits or plan. Your default if left out.",
		}, "card")},
	{"name": "card_message", "description": "Send a message to the agent in rush working on a kanban-code card. It goes when the agent's turn ends.",
		"inputSchema": schema(map[string]string{"card": "The card's id, id prefix, or name.", "text": "The message."}, "card", "text")},
}
