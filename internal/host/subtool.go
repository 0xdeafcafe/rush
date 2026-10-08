package host

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The agent tools (agtools.Agents) for one rush session: each agent it
// starts is a rush session of its own, its child by Meta spawnedBy, which
// the UI draws under the call that started it. The child's host keeps its
// answer (answer.json) and, when asked to (report), sends it to the parent
// as a message once it's done. See docs/subagents.md.

// subagents runs the agent tools for the rush session parent, of kind.
type subagents struct {
	parent string
	kind   agent.Kind
}

// AgentTools are the agent tools for rush session id, or nil when there's
// no session to start them from.
func AgentTools(id string) agtools.Agents {
	if id == "" {
		return nil
	}
	cfg, err := ReadConfig(id)
	if err != nil {
		return nil
	}
	return subagents{parent: id, kind: agent.Migrated(cfg.Kind)}
}

// own is whether p is one of the session's own subagent types (agentDefs),
// which its Agent tool starts rather than spawn_agent.
func (sa subagents) own(p pick) bool {
	return agent.ReadsAsClaude(sa.kind) && p.r.k == sa.kind && p.r.prov == string(sa.kind) && plainModel(p.model) && p.r.ready
}

func (sa subagents) Catalogue() string {
	var lines []string
	for _, p := range picks() {
		if sa.own(p) {
			continue
		}
		line := "- " + p.name + ": " + p.desc + " " + agent.ProviderLabel(p.r.prov) + " in " + agent.HarnessLabel(p.r.k) + "."
		if !p.r.ready {
			line += " Not available right now."
		}
		lines = append(lines, line)
	}
	return strings.Join(lines, "\n")
}

// find is the agent called name: one listed, or an agent or provider by
// its own name on its default model.
func find(name string) (pick, bool) {
	ps := picks()
	for _, p := range ps {
		if p.name == name {
			return p, true
		}
	}
	for _, p := range ps {
		if string(p.r.k) == name || p.r.prov == name {
			p.name, p.model = name, ""
			return p, true
		}
	}
	return pick{}, false
}

func (sa subagents) Spawn(in agtools.SpawnInput) (string, error) {
	if strings.TrimSpace(in.Prompt) == "" {
		return "", errors.New("spawn_agent needs a prompt: the whole task")
	}
	p, ok := find(in.Agent)
	if !ok {
		return "", fmt.Errorf("rush has no agent called %q: pick one of those spawn_agent lists", in.Agent)
	}
	if !p.r.ready {
		return "", fmt.Errorf("%s needs its account checked or refreshed before taking work: in rush, Settings, %s", p.name, p.r.name)
	}
	parent, _ := ReadConfig(sa.parent)
	info, _ := ReadInfo(sa.parent)
	st := state.Load().Config
	start := st.Dispatch.StartFor(string(p.r.k))
	cfg := Config{Cwd: or(info.Cwd, parent.Cwd), Prompt: in.Prompt, Name: NameFrom(in.Prompt),
		Model: or(in.Model, or(p.model, start.Model)), Effort: or(in.Effort, start.Effort), PermissionMode: start.Mode,
		IdleStop: Duration(st.Dispatch.Rest()), SystemPrompt: workPrompt, Meta: map[string]string{"spawnedBy": sa.parent}}
	if agent.Migrated(parent.Kind) == p.r.k {
		cfg.PermissionMode = or(info.PermissionMode, cfg.PermissionMode) // it works for its parent, with its trust
	}
	if string(p.r.k) == state.LoginsKind {
		cfg.Account = st.ActiveAccount().Profile()
	}
	if err := cfg.UseAgent(string(p.r.k)); err != nil {
		return "", err
	}
	_ = SignInIfOut(string(p.r.k), cfg.Account)
	cfg.fillIDs()
	cfg.PromptExchange = sa.exchange(cfg.ID, "request", in.Prompt)
	if in.Background {
		if err := report(cfg.ID, sa.parent); err != nil {
			return "", err
		}
	}
	since := time.Now()
	started, err := Spawn(cfg)
	if err != nil {
		return "", fmt.Errorf("%s couldn't start: %w", p.name, err)
	}
	if in.Background {
		return fmt.Sprintf("%s started in the background as agent %s. Its answer is sent to you as a message when it finishes; agent_result with this id asks sooner.", p.name, started.ID), nil
	}
	return awaitAnswer(started.ID, sa.parent, since, foreground)
}

func (sa subagents) Result(in agtools.ResultInput) (string, error) {
	if err := sa.mine(in.ID); err != nil {
		return "", err
	}
	return awaitAnswer(in.ID, sa.parent, time.Time{}, time.Duration(min(max(in.Wait, 0), 600))*time.Second)
}

func (sa subagents) Send(in agtools.SendInput) (string, error) {
	if err := sa.mine(in.ID); err != nil {
		return "", err
	}
	// Only a background follow-up reports back; a waited one is answered here.
	if err := report(in.ID, map[bool]string{true: sa.parent}[in.Background]); err != nil {
		return "", err
	}
	if err := Ensure(in.ID); err != nil {
		return "", err
	}
	c, err := Dial(in.ID)
	if err != nil {
		return "", err
	}
	since := time.Now()
	err = c.SendExchange(*sa.exchange(in.ID, "message", in.Prompt), false)
	_ = c.Close()
	if err != nil {
		return "", err
	}
	if in.Background {
		return "Sent. Its answer is sent to you as a message when it finishes.", nil
	}
	return awaitAnswer(in.ID, sa.parent, since, foreground)
}

// mine refuses an id that isn't one of the session's agents.
func (sa subagents) mine(id string) error {
	cfg, err := ReadConfig(id)
	if err != nil || cfg.Meta["spawnedBy"] != sa.parent {
		return fmt.Errorf("no agent %q was started by this session", id)
	}
	return nil
}

// exchange is the parent's message to its child, so the child's
// conversation says who it's from.
func (sa subagents) exchange(child, phase, text string) *event.Exchange {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return &event.Exchange{ID: hex.EncodeToString(b), Direction: "received", Phase: phase, Text: text, Sender: peer(sa.parent), Receiver: peer(child)}
}

// peer is rush session id as one end of an exchange.
func peer(id string) event.Peer {
	p := event.Peer{SessionID: id}
	if c, err := ReadConfig(id); err == nil {
		p.Kind, p.Name = string(agent.Migrated(c.Kind)), c.Name
	}
	return p
}

// foreground is how long a waited call waits for its agent; past it, the
// agent works on and agent_result asks again.
const foreground = 50 * time.Minute

// answer is what a session's host keeps of its last turn (answer.json).
type answer struct {
	At   time.Time `json:"at"`
	Text string    `json:"text,omitempty"`
	Err  string    `json:"err,omitempty"`
}

func answerPath(id string) string { return filepath.Join(dir(id), "answer.json") }
func reportPath(id string) string { return filepath.Join(dir(id), "report") }

// report has child send its answers to parent from now on; to none, "".
func report(child, parent string) error {
	if parent == "" {
		if err := os.Remove(reportPath(child)); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		return nil
	}
	if err := os.MkdirAll(dir(child), 0o700); err != nil {
		return err
	}
	return os.WriteFile(reportPath(child), []byte(parent), 0o600)
}

// awaitAnswer waits up to wait for agent id to finish a turn ended after
// since (any, when zero), and says how it stands. One whose turn hung is
// left to rush's retries and its answer sent to parent when it comes, so
// parent isn't held waiting on it.
func awaitAnswer(id, parent string, since time.Time, wait time.Duration) (string, error) {
	name := id
	if c, err := ReadConfig(id); err == nil {
		name = agentName(agent.Migrated(c.Kind)) + " (" + id + ")"
	}
	for deadline := time.Now().Add(wait); ; time.Sleep(500 * time.Millisecond) {
		info, err := ReadInfo(id)
		if err != nil {
			return "", fmt.Errorf("no agent %s: %w", id, err)
		}
		a, done := finished(id, info, since)
		switch {
		case done && a.Err != "":
			return "", fmt.Errorf("%s failed: %s", name, a.Err)
		case done:
			return name + " finished:\n\n" + or(strings.TrimSpace(a.Text), "(it said nothing)"), nil
		case !info.Sleeping && info.HostPID > 0 && !alive(info.HostPID):
			return "", fmt.Errorf("%s stopped: %s", name, or(info.Error, "its host went away"))
		case info.Retry != nil && info.Retry.Hung && !info.Retry.GaveUp:
			if err := report(id, parent); err != nil {
				return "", err
			}
			return name + " stalled: nothing came from its model for minutes, so rush stopped the turn and is retrying it in the background. " +
				"Its answer is sent to you as a message when it finishes. Check whether you still need it; if not, carry on without it.", nil
		case !time.Now().Before(deadline):
			what := or(info.Detail, info.State)
			if info.Needs != "" {
				what = "waiting for the user to allow " + info.Needs
			}
			return fmt.Sprintf("%s is still working (%s). Ask again with agent_result and wait_seconds.", name, what), nil
		}
	}
}

// finished is agent id's last answer, when it ended a turn after since and
// has nothing more to do: idle with nothing queued or running beside it,
// or stopped by a limit or by errors it gave up retrying.
func finished(id string, info Info, since time.Time) (answer, bool) {
	var a answer
	b, err := os.ReadFile(answerPath(id))
	if err != nil || jsonx.Unmarshal(b, &a) != nil || a.At.Before(since) {
		return a, false
	}
	switch {
	case info.Limit != nil:
		a.Err = "it hit a usage limit"
	case info.Retry != nil && info.Retry.GaveUp:
		a.Err = or(info.Retry.Reason, "it gave up retrying")
	case info.State == "stopped" || info.Sleeping:
	case info.State != "idle" || len(info.Queue) > 0 || len(info.Background) > 0:
		return a, false
	}
	return a, true
}

func agentName(k agent.Kind) string {
	if a, ok := agent.Get(k); ok {
		return a.Name()
	}
	return string(k)
}

// said keeps the latest words the agent itself (not a subagent of its)
// said, for the turn's answer. Called with mu held.
func (s *server) said(m event.Message) {
	if m.Role != "assistant" || m.Parent != "" {
		return
	}
	for _, p := range m.Parts {
		if p.Kind == event.Text && strings.TrimSpace(p.Text) != "" {
			s.lastSaid = p.Text
		}
	}
}

// turnDone keeps the turn's answer for whoever waits on it, and sends it to
// the session it reports to once there's nothing more to do. Called with
// mu held, once onTurnEnd has settled the state.
func (s *server) turnDone(e event.TurnEnd) {
	if s.cfg.Meta["spawnedBy"] == "" {
		return
	}
	a := answer{At: time.Now(), Text: or(s.lastSaid, e.Text), Err: e.Err}
	if e.Reason != "error" {
		a.Err = ""
	}
	s.lastSaid = ""
	if b, err := jsonx.Marshal(a); err == nil {
		_ = os.WriteFile(answerPath(s.cfg.ID), b, 0o600)
	}
	if _, done := finished(s.cfg.ID, s.info, a.At); !done {
		return
	}
	to, err := os.ReadFile(reportPath(s.cfg.ID))
	if err != nil || len(to) == 0 {
		return
	}
	text := agentName(agent.Migrated(s.cfg.Kind)) + " (agent " + s.cfg.ID + ") finished"
	if a.Err != "" {
		text += ", failing: " + a.Err
	} else {
		text += ":\n\n" + or(strings.TrimSpace(a.Text), "(it said nothing)")
	}
	child, parent := s.cfg.ID, string(to)
	go func() {
		if err := sendResult(child, parent, text); err != nil {
			fmt.Fprintln(os.Stderr, "rush: couldn't send the answer to", parent+":", err)
		}
	}()
}

// sendResult gives parent child's answer as a message from child, waking
// parent if it rests.
func sendResult(child, parent, text string) error {
	if err := Ensure(parent); err != nil {
		return err
	}
	c, err := Dial(parent)
	if err != nil {
		return err
	}
	defer c.Close()
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return c.SendExchange(event.Exchange{ID: hex.EncodeToString(b), Direction: "received", Phase: "result", Text: text,
		Sender: peer(child), Receiver: peer(parent)}, false)
}
