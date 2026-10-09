package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/plugind"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/ui"
)

const sessionUsage = `rush session: run rush-mode sessions without the view

  rush session start --cwd DIR [--agent KIND] [--profile P] [--session-id UUID] [--resume] [--name N]
        [--prompt-file F] [--image PATH]... [--env K=V]... [--meta k=v]...
        [--binary PATH] [--model M] [--effort E] [--permission-mode M] [--json]

  --agent is the agent to run: claude, codex, copilot, gemini, kimi, opencode
  or vibe. Without it, the session's profile picks: --profile names one,
  else the folder's rule or the default profile says.
  rush session send <id> [--now] [--image PATH]...   message text on stdin; into the
        turn under way, or with --now stopping it
  rush session interrupt <id>
  rush session stop <id>
  rush session watch <id> [--json] [--until-idle]   what it says and does, as it
        happens, from the start of the turn under way; --until-idle stops
        when the turn ends
  rush session info <id> [--json]
  rush session list [--json] [--meta k=v]...
`

// sessionView is a session as the session commands print it: its info,
// and whether its host is running.
type sessionView struct {
	host.Info
	Alive bool `json:"alive"`
}

func viewOf(i host.Info) sessionView { return sessionView{Info: i, Alive: host.Alive(i.HostPID)} }

// errNotFound is a session id with no session behind it.
var errNotFound = errors.New("not found")

// sessionCmd runs rush session <sub> and returns the exit code.
func sessionCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	if len(args) == 0 || args[0] == "-h" || args[0] == "--help" || args[0] == "help" {
		fmt.Fprint(stdout, sessionUsage)
		return 0
	}
	var (
		asJSON bool
		err    error
	)
	sub, rest := args[0], args[1:]
	switch sub {
	case "start":
		asJSON, err = sessionStart(rest, stdout)
	case "send":
		err = sessionSend(rest, stdin, stdout)
	case "interrupt":
		err = sessionControl(rest, stdout, false)
	case "stop":
		err = sessionControl(rest, stdout, true)
	case "watch":
		asJSON, err = sessionWatch(rest, stdout)
	case "info":
		asJSON, err = sessionInfo(rest, stdout)
	case "list":
		asJSON, err = sessionList(rest, stdout)
	default:
		err = fmt.Errorf("unknown session command %q\n\n%s", sub, sessionUsage)
	}
	if err == nil {
		return 0
	}
	if asJSON {
		writeJSON(stdout, map[string]string{"error": err.Error()})
	} else {
		fmt.Fprintln(stderr, "rush:", err)
	}
	return 1
}

func writeJSON(w io.Writer, v any) { _ = jsonx.Write(w, v) }

// multi is a flag given any number of times.
type multi []string

func (m *multi) String() string     { return strings.Join(*m, ",") }
func (m *multi) Set(v string) error { *m = append(*m, v); return nil }

// idAndFlags takes the session id, before or after the flags.
func idAndFlags(fs *flag.FlagSet, args []string) (string, error) {
	var id string
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		id, args = args[0], args[1:]
	}
	if err := fs.Parse(args); err != nil {
		return "", err
	}
	if id == "" && fs.NArg() > 0 {
		id = fs.Arg(0)
	}
	if id == "" {
		return "", fmt.Errorf("usage: rush session %s <id>", fs.Name())
	}
	return id, nil
}

func newFlags(name string) *flag.FlagSet {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	return fs
}

func pairs(kvs []string, what string) (map[string]string, error) {
	if len(kvs) == 0 {
		return nil, nil
	}
	m := map[string]string{}
	for _, kv := range kvs {
		k, v, ok := strings.Cut(kv, "=")
		if !ok || k == "" {
			return nil, fmt.Errorf("--%s wants key=value, not %q", what, kv)
		}
		m[k] = v
	}
	return m, nil
}

var uuidRe = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)

// sessionExists is whether id has a session folder.
func sessionExists(id string) bool {
	if id == "" || strings.ContainsAny(id, "/\\") || id == "." || id == ".." {
		return false
	}
	st, err := os.Stat(filepath.Join(host.Root(), id))
	return err == nil && st.IsDir()
}

// waitInfo waits for a host just started to publish its info.
func waitInfo(id string) (host.Info, error) {
	var (
		info host.Info
		err  error
	)
	for deadline := time.Now().Add(10 * time.Second); time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		if info, err = host.ReadInfo(id); err == nil && host.Alive(info.HostPID) {
			return info, nil
		}
	}
	if err == nil {
		err = fmt.Errorf("the host for %s stopped right after starting", id)
	}
	return info, err
}

func sessionStart(args []string, stdout io.Writer) (bool, error) {
	fs := newFlags("start")
	var (
		cwd, sessionID, name, promptFile, binary, model, effort, mode, kind, profile string
		resume, asJSON                                                               bool
		images, env, meta                                                            multi
	)
	fs.StringVar(&cwd, "cwd", "", "")
	fs.StringVar(&kind, "agent", "", "")
	fs.StringVar(&profile, "profile", "", "")
	fs.StringVar(&sessionID, "session-id", "", "")
	fs.BoolVar(&resume, "resume", false, "")
	fs.StringVar(&name, "name", "", "")
	fs.StringVar(&promptFile, "prompt-file", "", "")
	fs.Var(&images, "image", "")
	fs.Var(&env, "env", "")
	fs.Var(&meta, "meta", "")
	fs.StringVar(&binary, "binary", "", "")
	fs.StringVar(&model, "model", "", "")
	fs.StringVar(&effort, "effort", "", "")
	fs.StringVar(&mode, "permission-mode", "", "")
	fs.BoolVar(&asJSON, "json", false, "")
	if err := fs.Parse(args); err != nil {
		return hasFlag(args, "--json"), err
	}
	if fs.NArg() > 0 {
		return asJSON, fmt.Errorf("unexpected argument %q", fs.Arg(0))
	}
	if resume && sessionID == "" {
		return asJSON, errors.New("--resume needs --session-id")
	}
	if sessionID != "" && !uuidRe.MatchString(sessionID) {
		return asJSON, fmt.Errorf("--session-id %q is not a UUID", sessionID)
	}
	for _, kv := range env {
		if k, _, ok := strings.Cut(kv, "="); !ok || k == "" {
			return asJSON, fmt.Errorf("--env wants KEY=value, not %q", kv)
		}
	}
	metaMap, err := pairs(meta, "meta")
	if err != nil {
		return asJSON, err
	}
	var prompt string
	if promptFile != "" {
		var b []byte
		if promptFile == "-" {
			b, err = io.ReadAll(os.Stdin)
		} else {
			b, err = os.ReadFile(promptFile)
		}
		if err != nil {
			return asJSON, err
		}
		prompt = strings.TrimRight(string(b), "\n")
	}
	for i, p := range images {
		if abs, err := filepath.Abs(p); err == nil {
			images[i] = abs
		}
	}

	st := state.Load()
	d := st.Config.Dispatch
	show := func(info host.Info) (bool, error) {
		v := viewOf(info)
		if asJSON {
			writeJSON(stdout, v)
		} else {
			fmt.Fprintf(stdout, "%s %s %s\n", v.ID, v.State, v.Name)
		}
		return asJSON, nil
	}

	var cfg host.Config
	if sessionID != "" {
		id := host.ShortID(sessionID)
		if info, err := host.ReadInfo(id); err == nil {
			if host.Alive(info.HostPID) {
				return show(info) // already running: nothing to start
			}
			if !resume {
				return asJSON, fmt.Errorf("session %s exists and is stopped; pass --resume to bring it back", id)
			}
			// Back from its saved config, as the view's resume does.
			if saved, err := host.ReadConfig(id); err == nil {
				cfg = saved
			}
		}
		if cfg.ID == "" {
			cfg = host.Config{ID: id, SessionID: sessionID}
		}
		cfg.Resume = resume
	}
	if cwd != "" {
		abs, err := filepath.Abs(cwd)
		if err != nil {
			return asJSON, err
		}
		cfg.Cwd = abs
	}
	if cfg.Cwd == "" {
		return asJSON, errors.New("--cwd is required")
	}
	if st, err := os.Stat(cfg.Cwd); err != nil || !st.IsDir() {
		return asJSON, fmt.Errorf("--cwd %s is not a folder", cfg.Cwd)
	}
	if profile != "" {
		if _, ok := st.Config.ProfileNamed(profile); !ok {
			return asJSON, fmt.Errorf("no profile named %q", profile)
		}
	}
	if !cfg.Resume || cfg.Profile == "" {
		p := st.Config.ProfileFor(cfg.Cwd, profile)
		cfg.Profile = p.Name
		if kind == "" && !cfg.Resume {
			// The profile's first provider installed here: without the view's
			// readings of each account, it can't tell which are nearly out.
			if pick, ok := p.Pick(nil); ok {
				kind = pick.Kind
			}
		}
	}
	kind = or(kind, or(cfg.Kind, st.Config.DefaultAgent()))
	if kind == state.LoginsKind && cfg.Account.Dir == "" {
		cfg.Account = st.Config.ActiveAccount().Profile()
	}
	if err := cfg.UseAgent(kind); err != nil {
		return asJSON, err
	}
	_ = host.SignInIfOut(kind, cfg.Account) // its own error says more, if it's still out
	// Each agent starts with what its own Settings page says, unless told.
	start := d.StartFor(kind)
	cfg.Prompt, cfg.Images = prompt, images
	cfg.Lean, cfg.IdleStop = d.Lean, host.Duration(d.Rest())
	if cfg.LimitMode == "" && kind == state.LoginsKind { // Dispatch's own are that agent's
		cfg.LimitMode = d.OnLimit
	}
	cfg.Model = or(model, or(cfg.Model, start.Model))
	cfg.Effort = or(effort, or(cfg.Effort, start.Effort))
	cfg.PermissionMode = or(mode, or(cfg.PermissionMode, start.Mode))
	cfg.Binary = or(binary, cfg.Binary)
	if len(env) > 0 {
		cfg.Env = env
	}
	if metaMap != nil {
		if cfg.Meta == nil {
			cfg.Meta = map[string]string{}
		}
		for k, v := range metaMap {
			cfg.Meta[k] = v
		}
	}
	// Started from a rush session's shell, it's that session's subagent.
	if by := os.Getenv("RUSH_SESSION"); by != "" && !cfg.Resume && cfg.Meta["spawnedBy"] == "" {
		if cfg.Meta == nil {
			cfg.Meta = map[string]string{}
		}
		cfg.Meta["spawnedBy"] = by
	}
	cfg.Name = or(name, cfg.Name)
	if cfg.Name == "" {
		cfg.Name = firstWordsOf(prompt)
	}
	if cfg.Name == "" && len(images) > 0 {
		cfg.Name = "about " + filepath.Base(images[0])
	}
	if cfg.Name == "" {
		cfg.Name = "fresh session in " + filepath.Base(cfg.Cwd)
	}
	started, err := host.Spawn(cfg)
	if err != nil {
		return asJSON, err
	}
	info, err := waitInfo(started.ID)
	if err != nil {
		return asJSON, err
	}
	return show(info)
}

func hasFlag(args []string, f string) bool {
	for _, a := range args {
		if a == f || a == f+"=true" {
			return true
		}
	}
	return false
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// firstWordsOf is the first few words of a prompt, the name a session
// goes by until it names itself.
func firstWordsOf(text string) string {
	words := strings.Fields(text)
	if len(words) > 6 {
		words = words[:6]
	}
	n := strings.Join(words, " ")
	if r := []rune(n); len(r) > 48 {
		n = string(r[:47]) + "…"
	}
	return n
}

// sessionSend sends stdin to a session, resuming it first if its host is
// not running.
func sessionSend(args []string, stdin io.Reader, stdout io.Writer) error {
	fs := newFlags("send")
	var (
		now    bool
		images multi
	)
	fs.BoolVar(&now, "now", false, "")
	fs.Var(&images, "image", "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		return err
	}
	if !sessionExists(id) {
		return fmt.Errorf("session %s %w", id, errNotFound)
	}
	b, err := io.ReadAll(stdin)
	if err != nil {
		return err
	}
	text := strings.TrimRight(string(b), " \t\r\n")
	if text == "" && len(images) == 0 {
		return errors.New("nothing to send: write the message on stdin")
	}
	for i, p := range images {
		if abs, err := filepath.Abs(p); err == nil {
			images[i] = abs
		}
	}
	exchange := outgoingExchange(id, "message", text, images)
	was, readErr := host.ReadInfo(id)
	resumed := readErr != nil || was.Sleeping || !host.Alive(was.HostPID)
	if err := host.Ensure(id); err != nil {
		return err
	}
	c, err := host.Dial(id)
	if err != nil {
		return err
	}
	defer c.Close()
	// The host replays the conversation first and ends the replay with its
	// info: what comes after that answers this send.
	peerProto := 0
	if err := awaitLine(c, 10*time.Second, func(ev any) bool {
		i, ok := ev.(host.InfoEvent)
		if ok {
			peerProto = i.Info.Proto
		}
		return ok
	}); err != nil {
		return err
	}
	if exchange != nil && peerProto < 8 {
		fmt.Fprintln(stdout, "warning: receiver runs an older host; agent attribution needs a host restart")
		exchange = nil
	}
	// Into the turn under way, not after it: another agent's message
	// shouldn't wait on the queue. Idle, or where the agent can't take one
	// mid-turn, the host sends it as any message.
	switch {
	case exchange != nil:
		err = c.SendExchange(*exchange, now)
	case now && len(images) > 0:
		err = c.SendImages(text, images, now)
	case now:
		err = c.SendNow(text)
	default:
		err = c.SendGuide(text, images)
	}
	if err != nil {
		return err
	}
	var failed error
	waitErr := awaitLine(c, 5*time.Second, func(ev any) bool {
		switch e := ev.(type) {
		case host.ErrorEvent:
			failed = errors.New(e.Error)
			return true
		case host.Sent:
			return true
		case host.InfoEvent:
			if exchange != nil {
				for _, queued := range e.Info.QueueExchanges {
					if queued != nil && queued.ID == exchange.ID {
						return true
					}
				}
				return false
			}
			return len(e.Info.Queue) > 0 && e.Info.Queue[len(e.Info.Queue)-1] == text
		}
		return false
	})
	if waitErr != nil {
		return waitErr
	}
	if failed != nil {
		return failed
	}
	if err := mirrorOutgoing(exchange); err != nil {
		fmt.Fprintf(stdout, "warning: delivered to %s, but sender transcript update failed: %v\n", id, err)
	}
	if resumed {
		fmt.Fprintf(stdout, "resumed %s with the message\n", id)
	} else {
		fmt.Fprintf(stdout, "sent to %s\n", id)
	}
	return nil
}

// awaitLine reads host lines until want accepts one.
func awaitLine(c *host.Client, d time.Duration, want func(any) bool) error {
	timeout := time.After(d)
	for {
		select {
		case line, ok := <-c.Lines:
			if !ok {
				return errors.New("the host closed the connection")
			}
			if ev, err := host.Decode(line); err == nil && want(ev) {
				return nil
			}
		case <-timeout:
			return errors.New("the host did not answer in time")
		}
	}
}

// sessionControl interrupts a session's turn, or stops the session.
func sessionControl(args []string, stdout io.Writer, stop bool) error {
	name := "interrupt"
	if stop {
		name = "stop"
	}
	id, err := idAndFlags(newFlags(name), args)
	if err != nil {
		return err
	}
	if !sessionExists(id) {
		return fmt.Errorf("session %s %w", id, errNotFound)
	}
	info, err := host.ReadInfo(id)
	if err != nil || !host.Alive(info.HostPID) {
		if stop {
			fmt.Fprintf(stdout, "%s is already stopped\n", id)
			return nil
		}
		return fmt.Errorf("session %s is not running", id)
	}
	c, err := host.Dial(id)
	if err != nil {
		return err
	}
	defer c.Close()
	if !stop {
		if err := c.Interrupt(); err != nil {
			return err
		}
		fmt.Fprintf(stdout, "interrupted %s\n", id)
		return nil
	}
	if err := c.Stop(); err != nil {
		return err
	}
	for deadline := time.Now().Add(15 * time.Second); ; time.Sleep(50 * time.Millisecond) {
		if !host.Alive(info.HostPID) {
			break
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("the host for %s didn't stop", id)
		}
	}
	fmt.Fprintf(stdout, "stopped %s\n", id)
	return nil
}

// sessionWatch prints what a session says and does as it happens, as
// plugins see it with sessions.subscribe: with --json a line of JSON an
// event, {"event": type, "session": id, ...its fields}.
func sessionWatch(args []string, stdout io.Writer) (bool, error) {
	fs := newFlags("watch")
	var asJSON, untilIdle bool
	fs.BoolVar(&asJSON, "json", false, "")
	fs.BoolVar(&untilIdle, "until-idle", false, "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		return asJSON || hasFlag(args, "--json"), err
	}
	if !sessionExists(id) {
		return asJSON, fmt.Errorf("session %s %w", id, errNotFound)
	}
	if info, err := host.ReadInfo(id); err != nil || !host.Alive(info.HostPID) {
		return asJSON, fmt.Errorf("session %s is not running", id)
	}
	c, err := host.Dial(id)
	if err != nil {
		return asJSON, err
	}
	defer c.Close()
	var (
		done, busy, open bool // open: a line of streamed words not yet ended
		failed           error
		streamed         string // words printed as written, not yet come whole
	)
	end := func(err error) {
		done, failed = true, err
		c.Close() // and Follow returns once it has read what's buffered
	}
	line := func(format string, a ...any) {
		if open {
			fmt.Fprintln(stdout)
			open = false
		}
		fmt.Fprintf(stdout, format+"\n", a...)
	}
	idle := func(state string) bool { return state == "idle" || state == "stopped" }
	plugind.Follow(c.Lines, true, func(ev map[string]any) {
		if done {
			return
		}
		typ, _ := ev["type"].(string)
		text, _ := ev["text"].(string)
		state, _ := ev["state"].(string)
		isErr, _ := ev["isError"].(bool)
		if asJSON {
			delete(ev, "type")
			ev["event"], ev["session"] = typ, id
			writeJSON(stdout, ev)
		} else {
			switch typ {
			case "info":
				needs, _ := ev["needs"].(string)
				detail, _ := ev["detail"].(string)
				line("· %s%s", state, prefixed(": ", or(needs, detail)))
			case "sent":
				line("> %s", text)
			case "delta":
				fmt.Fprint(stdout, text)
				streamed += text
				open = !strings.HasSuffix(text, "\n")
			case "text":
				// Streamed already, the words aren't printed again.
				if rest, ok := strings.CutPrefix(streamed, text); ok {
					if streamed = rest; streamed == "" && open {
						fmt.Fprintln(stdout)
						open = false
					}
				} else {
					streamed = ""
					line("%s", text)
				}
			case "tool":
				name, _ := ev["name"].(string)
				doing, _ := ev["doing"].(string)
				line("→ %s", or(doing, name))
			case "result":
				if isErr {
					line("· failed: %s", text)
				} else {
					line("· done, $%.2f", ev["costUsd"])
				}
			}
		}
		switch {
		case !untilIdle:
		case typ == "result" && isErr:
			end(fmt.Errorf("the turn failed: %s", text))
		// Idle when it started is waiting for a turn, not the end of one.
		case typ == "result", typ == "info" && busy && idle(state):
			end(nil)
		}
		if typ == "info" {
			busy = !idle(state)
		}
	})
	if done {
		return asJSON, failed
	}
	if asJSON {
		writeJSON(stdout, map[string]string{"event": "closed", "session": id})
	} else {
		line("· closed")
	}
	if untilIdle {
		return asJSON, fmt.Errorf("session %s closed before its turn ended", id)
	}
	return asJSON, nil
}

// prefixed is s after p, or nothing when s is.
func prefixed(p, s string) string {
	if s == "" {
		return ""
	}
	return p + s
}

func sessionInfo(args []string, stdout io.Writer) (bool, error) {
	fs := newFlags("info")
	var asJSON bool
	fs.BoolVar(&asJSON, "json", false, "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		return asJSON || hasFlag(args, "--json"), err
	}
	if !sessionExists(id) {
		return asJSON, errNotFound
	}
	info, err := host.ReadInfo(id)
	if err != nil {
		return asJSON, errNotFound
	}
	v := viewOf(info)
	if asJSON {
		writeJSON(stdout, v)
		return true, nil
	}
	fmt.Fprintf(stdout, "%s %s %s\n  cwd %s\n  session %s\n", v.ID, v.State, v.Name, v.Cwd, v.SessionID)
	return false, nil
}

func sessionList(args []string, stdout io.Writer) (bool, error) {
	fs := newFlags("list")
	var (
		asJSON bool
		meta   multi
	)
	fs.BoolVar(&asJSON, "json", false, "")
	fs.Var(&meta, "meta", "")
	if err := fs.Parse(args); err != nil {
		return hasFlag(args, "--json"), err
	}
	want, err := pairs(meta, "meta")
	if err != nil {
		return asJSON, err
	}
	out := []sessionView{}
	for _, i := range host.List() {
		if matches(i.Meta, want) {
			out = append(out, viewOf(i))
		}
	}
	if asJSON {
		writeJSON(stdout, out)
		return true, nil
	}
	for _, v := range out {
		fmt.Fprintf(stdout, "%s  %-8s %s\n", v.ID, v.State, v.Name)
	}
	return false, nil
}

func matches(have, want map[string]string) bool {
	for k, v := range want {
		if got, ok := have[k]; !ok || got != v {
			return false
		}
	}
	return true
}

// openHosted is rush open <id> [--hosted]: the view of that one session
// alone. Without --hosted it is the same view.
func openHosted(args []string) error {
	fs := newFlags("open")
	fs.Bool("hosted", true, "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		return err
	}
	if !sessionExists(id) {
		return fmt.Errorf("session %s %w", id, errNotFound)
	}
	if _, err := host.ReadInfo(id); err != nil {
		return fmt.Errorf("session %s %w", id, errNotFound)
	}
	viewGC()
	state.WriteBehind() // the UI goroutine never waits on a save
	agent.NeverWait()   // nor on looking for agents' programs
	p := tea.NewProgram(ui.NewHosted(state.Load(), version, id), tea.WithFPS(120))
	_, err = p.Run()
	ui.StopSchemeReports()
	_ = state.Flush()
	return err
}
