package host

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/failure"
	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/state"
)

// The agent tools (agtools.Agents) for one rush session: each agent it
// starts is a rush session of its own, its child by Meta spawnedBy, which
// the UI draws under the call that started it. The child's host keeps its
// answer (answer.json) and, when asked to (report), sends it to the parent
// as a message once it's done. See docs/subagents.md.

// subagents runs the agent tools for the rush session parent, of kind.
type subagents struct {
	parent   string
	kind     agent.Kind
	headless bool
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
	return subagents{parent: id, kind: agent.Migrated(cfg.Kind), headless: cfg.Headless}
}

// Headless is whether no one watches the session in rush (Config.Headless).
func (sa subagents) Headless() bool { return sa.headless }

// own is whether p is one of the session's own subagent types (agentDefs),
// which its Agent tool starts rather than spawn_agent.
func (sa subagents) own(p pick) bool {
	return agent.ReadsAsClaude(sa.kind) && p.r.k == sa.kind && p.r.prov == string(sa.kind) && plainModel(p.model) && p.r.ready
}

// SetView shows the agent's view of name in rush's panel.
func (sa subagents) SetView(name string, raw jsontext.Value) (string, error) {
	var v View
	if err := jsonx.Unmarshal(raw, &v); err != nil {
		return "", fmt.Errorf("its view: %w", err)
	}
	if strings.Contains(name, "--") {
		return "", errors.New("a view's name has no -- in it: those are plugins'")
	}
	v.Name, v.By = name, ""
	if err := SaveView(sa.parent, v); err != nil {
		return "", err
	}
	return "Shown beside the conversation as " + name + ". Set it again as things change; clear it once it's no use.", nil
}

// ClearView puts the agent's view of name away among the past ones.
func (sa subagents) ClearView(name string) error { return DropView(sa.parent, name) }

func (sa subagents) Catalogue() string {
	var lines, own []string
	ps := picks(agent.HarnessOf(sa.kind))
	for i := range ps {
		p := ps[i]
		if sa.own(p) {
			own = append(own, p.name)
			continue
		}
		var line strings.Builder
		line.WriteString("- " + p.name)
		if p.model != "" && p.model != p.name {
			line.WriteString(" (" + p.model + ")")
		}
		line.WriteString(": " + p.desc + " " + agent.ProviderLabel(p.r.prov) + " in " + agent.HarnessLabel(p.r.k) + ".")
		if !p.r.ready {
			line.WriteString(" Not available right now.")
		}
		lines = append(lines, line.String())
	}
	if len(own) > 0 {
		lines = append(lines, "Your own models ("+strings.Join(own, ", ")+") aren't listed: they're your subagent types, for your Agent tool.")
	}
	return strings.Join(lines, "\n")
}

func (sa subagents) Spawn(in agtools.SpawnInput) (string, error) {
	if strings.TrimSpace(in.Prompt) == "" {
		return "", errors.New("spawn_agent needs a prompt: the whole task")
	}
	leader := agent.HarnessOf(sa.kind)
	ps := picks(leader)
	p, model, ok := find(ps, in.Agent, in.Model)
	if !ok {
		return "", noAgent(in.Agent, in.Model, ps)
	}
	if model != "" && p.r.cell != "" && !state.Load().Config.Calls(p.r.cell, agent.HarnessOf(p.r.k), model) {
		return "", fmt.Errorf("%s may not call %s in %s: Settings › Config keeps it to others", agent.ProviderLabel(p.r.prov), model, agent.HarnessLabel(p.r.k))
	}
	if !p.r.ready {
		return "", fmt.Errorf("%s needs its account checked or refreshed before taking work: in rush, Settings, %s", p.name, p.r.name)
	}
	parent, _ := ReadConfig(sa.parent)
	info, _ := ReadInfo(sa.parent)
	st := state.Load().Config
	start := st.Dispatch.ResolvedFor(string(p.r.k))
	cfg := Config{Cwd: or(info.Cwd, parent.Cwd), Prompt: in.Prompt, Name: NameFrom(in.Prompt),
		Model: or(model, or(p.model, start.Model)), Effort: or(in.Effort, start.Effort), PermissionMode: start.Mode,
		SystemPrompt: workPrompt, Meta: map[string]string{"spawnedBy": sa.parent}}
	if in.Step != "" {
		cfg.Meta["spawnStep"] = in.Step // the call that asked: the UI draws it there
	}
	if agent.Migrated(parent.Kind) == p.r.k {
		cfg.PermissionMode = or(info.PermissionMode, cfg.PermissionMode) // it works for its parent, with its trust
	} else if cfg.PermissionMode == "" {
		// On another harness it has the same trust in that harness's
		// words: a parent that doesn't ask doesn't hand you a child that
		// asks before every tool. A start mode set for it wins.
		ch, _ := agent.ChoicesOf(p.r.k)
		cfg.PermissionMode = agent.TrustedAs(info.PermissionMode, ch.Modes)
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
	// Logged before its host starts, whose lines it causes.
	cfg.Cause = events.ID()
	_ = os.MkdirAll(dir(cfg.ID), 0o700) // its own log is in its folder
	logEvent(cfg.ID, "spawned", map[string]any{"ev": cfg.Cause, "by": "spawn_agent", "spawnedBy": sa.parent,
		"agent": cfg.Kind, "model": cfg.Model, "background": in.Background, "prompt": clip(in.Prompt)})
	started, err := Spawn(cfg)
	if err != nil {
		logEvent(cfg.ID, "spawn-failed", map[string]any{"by": "spawn_agent", "spawnedBy": sa.parent, "agent": p.name, "err": err.Error(), "cause": cfg.Cause})
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
	// Waited on here, its answer comes here: the message it would also send
	// is held back, and owed again only if this wait ends without it.
	to, _ := os.ReadFile(reportPath(in.ID))
	owed := string(to) == sa.parent
	if owed {
		_ = report(in.ID, "")
	}
	out, done, err := await(in.ID, sa.parent, time.Time{}, time.Duration(min(max(in.Wait, 0), 600))*time.Second)
	// ponytail: a turn ending between the last look and this restore isn't
	// sent; re-check finished here if that ever bites.
	if owed && !done {
		if _, gone := os.Stat(reportPath(in.ID)); gone != nil {
			_ = report(in.ID, sa.parent)
		}
	}
	return out, err
}

func (sa subagents) Send(in agtools.SendInput) (string, error) {
	if err := sa.mine(in.ID); err != nil {
		return "", err
	}
	// Only a background follow-up reports back; a waited one is answered here.
	if err := report(in.ID, map[bool]string{true: sa.parent}[in.Background]); err != nil {
		return "", err
	}
	since := time.Now()
	if err := Deliver(in.ID, *sa.exchange(in.ID, "message", in.Prompt), false); err != nil {
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
// since (any, when zero), and says how it stands.
func awaitAnswer(id, parent string, since time.Time, wait time.Duration) (string, error) {
	out, _, err := await(id, parent, since, wait)
	return out, err
}

// await is awaitAnswer, saying too whether the turn's answer was given.
func await(id, parent string, since time.Time, wait time.Duration) (_ string, done bool, _ error) {
	name := id
	if c, err := ReadConfig(id); err == nil {
		name = agentName(agent.Migrated(c.Kind)) + " (" + id + ")"
	}
	for deadline := time.Now().Add(wait); ; time.Sleep(500 * time.Millisecond) {
		info, err := ReadInfo(id)
		if err != nil {
			return "", false, fmt.Errorf("no agent %s: %w", id, err)
		}
		a, done := finished(id, info, since)
		switch {
		case done && a.Err != "":
			_ = report(id, "") // answered here, so not as a message too
			return "", true, fmt.Errorf("%s%s failed: %s", name, titled(info.Name), a.Err)
		case done:
			_ = report(id, "")
			return name + " finished:\n\n" + or(strings.TrimSpace(a.Text), "(it said nothing)"), true, nil
		case !info.Sleeping && info.HostPID > 0 && !alive(info.HostPID):
			return "", false, fmt.Errorf("%s stopped: %s", name, or(info.Error, "its host went away"))
		case info.Limit != nil && info.Limit.Continue:
			if err := report(id, parent); err != nil {
				return "", false, err
			}
			return fmt.Sprintf("%s hit a usage limit and carries on when it resets at %s. Its answer is sent to you as a message when it finishes.",
				name, info.Limit.ResetsAt.Local().Format("15:04")), false, nil
		case info.State == "blocked" && info.Needs != "":
			return fmt.Sprintf("%s is waiting for the user to allow %s. Ask again with agent_result and wait_seconds once they have.", name, info.Needs), false, nil
		case !time.Now().Before(deadline):
			return fmt.Sprintf("%s is still working (%s). Ask again with agent_result and wait_seconds.", name, or(info.Detail, info.State)), false, nil
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
	case info.Retry != nil && !info.Retry.GaveUp, info.Limit != nil && info.Limit.Continue:
		return a, false // rush tries it again: that answer isn't its last
	case info.Limit != nil:
		a.Err = "it hit a usage limit"
	case info.Retry != nil && info.Retry.GaveUp:
		a.Err = FailReport(info.Kind, or(info.Retry.Reason, "it gave up retrying"), info.Retry.LastStep, info.Retry.Attempt-1)
	case info.State == "stopped" || info.Sleeping:
	case info.State != "idle" || len(info.Queue) > 0 || len(info.Background) > 0:
		return a, false
	}
	return a, true
}

// FailReport is how a run failed, as the session or room it reports to is told:
// the error (as failure.Summary says it, when it's the provider's), the
// last step the run took, the retries rush made, and for a login that
// stopped working, how to sign in again.
func FailReport(kind, err, step string, retries int) string {
	k := failure.KindOf(err)
	out := firstLine(err)
	if k != failure.Fatal {
		out = "provider error: " + failure.Summary(err)
	}
	if step != "" {
		out += ". Its last step: " + step
	}
	if retries > 0 {
		out += fmt.Sprintf(". rush retried it %d times", retries)
	}
	if k == failure.Auth {
		out += ". To sign in again, " + loginHint(agent.Migrated(kind))
	}
	return out
}

// loginHint is how to sign agent k in again: its own command, where it
// has one, else rush's settings.
func loginHint(k agent.Kind) string {
	if h, ok := agent.As[agent.LoginHinter](k); ok {
		return h.LoginHint()
	}
	return "use rush's Settings → Providers"
}

// titled is a failed run's title, for the session it reports to to know
// which of its runs it was: ` "Fix the login form"`, or nothing.
func titled(name string) string {
	if name == "" {
		return ""
	}
	return ` "` + name + `"`
}

func agentName(k agent.Kind) string {
	if a, ok := agent.Get(k); ok {
		return a.Name()
	}
	return string(k)
}

// said keeps the latest words the agent itself (not a subagent of its)
// said, for the turn's answer and its decisions. Called with mu held.
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
		s.lastSaid = "" // the next turn's words are its own
		return
	}
	// A turn that ends with more to do (a queued message) is answered with
	// the next, not lost to it.
	text := or(s.lastSaid, e.Text)
	if s.owed != "" {
		text = strings.TrimSpace(s.owed + "\n\n" + text)
	}
	a := answer{At: time.Now(), Text: text}
	if e.Err != "" && e.Reason != "interrupted" { // a "done" can carry an error too (Claude's is_error)
		a.Err = FailReport(s.cfg.Kind, e.Err, s.lastStep, 0)
	} else {
		s.lastStep = "" // the next failure's last step is the next run's
	}
	s.lastSaid, s.owed = "", ""
	if b, err := jsonx.Marshal(a); err == nil {
		_ = os.WriteFile(answerPath(s.cfg.ID), b, 0o600)
	}
	if _, done := finished(s.cfg.ID, s.info, a.At); !done {
		if len(s.info.Queue) > 0 {
			s.owed = a.Text
		}
		return
	}
	s.reportAnswer()
}

// reportAnswer sends the last answer to the session it reports to, once
// there's nothing more to do, and only once. Called with mu held.
func (s *server) reportAnswer() {
	a, done := finished(s.cfg.ID, s.info, time.Time{})
	if !done {
		return
	}
	to, err := os.ReadFile(reportPath(s.cfg.ID))
	if err != nil || len(to) == 0 {
		return
	}
	_ = report(s.cfg.ID, "")
	text := agentName(agent.Migrated(s.cfg.Kind)) + " (agent " + s.cfg.ID + ")"
	if a.Err != "" {
		text += titled(s.info.Name) + " finished, failing: " + a.Err
	} else {
		text += " finished:\n\n" + or(strings.TrimSpace(a.Text), "(it said nothing)")
	}
	child, parent := s.cfg.ID, string(to)
	s.reporting++ // the host doesn't sleep until it's gone (answerGiven)
	go func() {
		if err := sendResult(child, parent, text); err != nil {
			fmt.Fprintln(os.Stderr, "rush: couldn't send the answer to", parent+":", err)
		}
		s.mu.Lock()
		s.reporting--
		s.mu.Unlock()
	}()
}

// sendResult gives parent child's answer as a message from child, waking
// parent if it rests.
func sendResult(child, parent, text string) error {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return Deliver(parent, event.Exchange{ID: hex.EncodeToString(b), Direction: "received", Phase: "result", Text: text,
		Sender: peer(child), Receiver: peer(parent)}, false)
}
