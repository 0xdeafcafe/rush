package main

import (
	"bytes"
	"encoding/json/jsontext"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/charmbracelet/x/term"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// rush spawn <program> <args…> is what a session's stand-in for an
// agent's program runs (see host.WriteShims). A run rush can host (codex
// exec, claude -p, vibe -p) runs as a rush session, so you can send to it
// while it works, printing what the program would have. Anything else, or
// a flag rush can't print faithfully, runs the real program as it was
// asked.

// workRun is a run rush can host.
type workRun struct {
	kind                agent.Kind
	prompt, cwd         string
	model, effort, mode string
	json, partial       bool   // codex --json; claude --include-partial-messages
	format              string // claude's --output-format: text, json, stream-json
	lastMsg             string // codex -o
	skipGit             bool
	stdinNote           string // what the program says as it reads stdin
}

// spawnCmd runs rush spawn and returns the exit code.
func spawnCmd(args []string) int {
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "usage: rush spawn <program> [args...]")
		return 2
	}
	prog, rest := args[0], args[1:]
	path := os.Getenv("PATH")
	// What this runs (a host, the real program's lookups) finds the real
	// programs; the real one run in its place keeps PATH as it was.
	_ = os.Setenv("PATH", host.WithoutShims(path))
	r, ok := parseRun(prog, rest)
	// RUSH_AGENT=ollama claude -p … runs on another provider through the
	// same harness; its own spawns choose again.
	rider := riderOf(agent.Kind(os.Getenv("RUSH_AGENT")), agent.Kind(prog))
	if ok || rider != "" {
		_ = os.Unsetenv("RUSH_AGENT")
	}
	// Run as itself too, the program finds the sign-in rush keeps for it.
	if a, found := agent.Get(agent.Kind(prog)); found && rider == "" {
		if ps := agent.ProfilesOf(a); len(ps) > 0 {
			if err := host.SignInIfOut(prog, ps[0]); err != nil {
				fmt.Fprintln(os.Stderr, "rush:", err)
			}
		}
	}
	if code, hosted := spawnHosted(prog, rest, path, r, ok && os.Getenv("RUSH_NO_SHIM") == "", rider); hosted {
		return code
	}
	if rider != "" {
		// The real program would run it on its own provider instead.
		fmt.Fprintf(os.Stderr, "rush: couldn't run this on %s (RUSH_AGENT=%s): only a run rush can host goes to it\n", agentName(rider), rider)
		return 1
	}
	return runReal(prog, rest, path, nil)
}

// spawnHosted hosts r when it can (host), with any prompt it's fed on
// stdin; false when the real program should run instead.
func spawnHosted(prog string, rest []string, path string, r workRun, can bool, rider agent.Kind) (int, bool) {
	if !can {
		return 0, false
	}
	if rider != "" {
		r.kind = rider
	}
	if k, ok := agent.Get(r.kind); !ok || !agent.Installed(k.Kind()) {
		return 0, false
	}
	in, fed, waited := readStdin()
	if len(in) > 0 {
		p, ok := withStdin(r, in)
		if !ok && rider != "" {
			return 0, false
		}
		if !ok {
			// The program adds what it's fed to the prompt: it does that itself.
			return runReal(prog, rest, path, io.MultiReader(bytes.NewReader(in), os.Stdin)), true
		}
		r.prompt = p
	}
	if r.prompt == "" {
		return 0, false // the program says what it wants
	}
	if fed && r.stdinNote != "" {
		fmt.Fprintln(os.Stderr, r.stdinNote)
	}
	if waited && harness(r.kind) == "claude" { // migration: per-agent CLI parsing and output move behind the adapters
		fmt.Fprintln(os.Stderr, "Warning: no stdin data received in 3s, proceeding without it. If piping from a slow command, redirect stdin explicitly: < /dev/null to skip, or wait longer.")
	}
	return hostRun(r, os.Stdout, os.Stderr)
}

// riderOf is k when it's a provider riding harness's program, else "".
func riderOf(k, harnessKind agent.Kind) agent.Kind {
	if a, found := agent.Get(k); found && k != harnessKind {
		if _, rides := a.(agent.Rider); rides && harness(k) == harnessKind {
			return k
		}
	}
	return ""
}

func agentName(k agent.Kind) string {
	if a, ok := agent.Get(k); ok {
		return a.Name()
	}
	return string(k)
}

// maxStdin is the most readStdin takes in; a prompt that long runs the
// real program, which reads the rest.
const maxStdin = 1 << 20

// withStdin is r's prompt with what it was fed on stdin, as its program
// puts them together; false when rush can't say how it would.
func withStdin(r workRun, in []byte) (string, bool) {
	s := string(in)
	if len(in) >= maxStdin || strings.TrimSpace(s) == "" && r.prompt == "" {
		return "", false
	}
	switch harness(r.kind) { // migration: per-agent CLI parsing and output move behind the adapters
	case "claude": // migration: as above
		if r.prompt == "" {
			return s, true
		}
		return r.prompt + "\n" + s, true
	case "codex":
		if r.prompt == "" {
			return s, true
		}
		if !strings.HasSuffix(s, "\n") {
			s += "\n"
		}
		return r.prompt + "\n\n<stdin>\n" + s + "</stdin>", true
	}
	return "", false
}

// parseRun is the run args ask prog for, when rush can host it and print
// it as prog would.
func parseRun(prog string, args []string) (workRun, bool) {
	// As a shell command: a word the shell would take apart, quoted.
	words := make([]string, 0, 1+len(args))
	words = append(words, prog)
	for _, a := range args {
		if a == "" || strings.ContainsFunc(a, func(r rune) bool { return !strings.ContainsRune(shellSafe, r) }) {
			a = "'" + strings.ReplaceAll(a, "'", `'\''`) + "'"
		}
		words = append(words, a)
	}
	sp, ok := convo.SpawnOf(strings.Join(words, " "))
	if !ok {
		return workRun{}, false
	}
	switch sp.Kind {
	case "codex":
		return codexRun(args)
	case "claude": // migration: per-agent CLI parsing and output move behind the adapters
		return claudeRun(args)
	}
	rd, ok := agent.As[agent.OnceReader](sp.Kind)
	if !ok {
		return workRun{}, false
	}
	o, ok := rd.ReadOnce(args)
	return workRun{kind: sp.Kind, prompt: o.Prompt, model: o.Model, mode: o.Mode, cwd: o.Cwd, format: "text"}, ok
}

const shellSafe = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./=:@,+%"

// flagArgs walks args: each flag with its value (for those that take
// one, as --x=v or --x v), and each bare word. Any flag it isn't told
// about makes it false.
func flagArgs(args []string, valued, bare map[string]bool, each func(name, val string) bool, word func(string) bool) bool {
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" { // the rest are words, a prompt starting with - too
			for _, w := range args[i+1:] {
				if !word(w) {
					return false
				}
			}
			return true
		}
		if !strings.HasPrefix(a, "-") || a == "-" {
			if !word(a) {
				return false
			}
			continue
		}
		name, val, eq := strings.Cut(a, "=")
		switch {
		case valued[name] && !eq:
			if i+1 >= len(args) {
				return false
			}
			i++
			val = args[i]
		case bare[name] && !eq:
		case valued[name]:
		default:
			return false
		}
		if !each(name, val) {
			return false
		}
	}
	return true
}

var (
	codexValued = map[string]bool{"-m": true, "--model": true, "-s": true, "--sandbox": true, "-C": true, "--cd": true,
		"-o": true, "--output-last-message": true, "--color": true, "-c": true, "--config": true}
	codexBare = map[string]bool{"--full-auto": true, "--dangerously-bypass-approvals-and-sandbox": true, "--yolo": true,
		"--skip-git-repo-check": true, "--json": true, "--experimental-json": true}
	// codexModes are codex's sandboxes as the modes its adapter takes.
	codexModes = map[string]string{"read-only": "read-only", "workspace-write": "auto", "danger-full-access": "full-access"}
)

// codexRun reads codex [flags] exec [flags] PROMPT.
func codexRun(args []string) (workRun, bool) {
	r := workRun{kind: "codex", format: "text"}
	sub := false
	var words []string
	ok := flagArgs(args, codexValued, codexBare, func(name, val string) bool {
		switch name {
		case "-m", "--model":
			r.model = val
		case "-s", "--sandbox":
			r.mode = codexModes[val]
			return r.mode != ""
		case "--full-auto":
			r.mode = "auto"
		case "--dangerously-bypass-approvals-and-sandbox", "--yolo":
			r.mode = "full-access"
		case "-C", "--cd":
			r.cwd = val
		case "--skip-git-repo-check":
			r.skipGit = true
		case "--json", "--experimental-json":
			r.json = true
		case "-o", "--output-last-message":
			r.lastMsg = val
		case "-c", "--config":
			// Only the effort maps onto a session's own settings.
			k, v, _ := strings.Cut(val, "=")
			if strings.TrimSpace(k) != "model_reasoning_effort" {
				return false
			}
			r.effort = strings.Trim(strings.TrimSpace(v), `"'`)
		}
		return true
	}, func(w string) bool {
		if !sub {
			sub = w == "exec" || w == "e"
			return sub
		}
		words = append(words, w)
		return true
	})
	// exec resume and exec review are runs of another kind.
	if !ok || !sub || len(words) > 1 || len(words) == 1 && (words[0] == "resume" || words[0] == "review" || words[0] == "help") {
		return workRun{}, false
	}
	// With no prompt, or -, it's all on stdin.
	switch {
	case len(words) == 0:
		r.stdinNote = "Reading prompt from stdin..."
	case words[0] != "-":
		r.prompt, r.stdinNote = words[0], "Reading additional input from stdin..."
	}
	return r, true
}

var (
	claudeValued = map[string]bool{"--model": true, "--output-format": true, "--permission-mode": true, "--effort": true}
	claudeBare   = map[string]bool{"-p": true, "--print": true, "--verbose": true, "--include-partial-messages": true,
		"--dangerously-skip-permissions": true}
)

// claudeRun reads claude -p [flags] PROMPT.
func claudeRun(args []string) (workRun, bool) {
	r := workRun{kind: "claude", format: "text"} // migration: per-agent CLI parsing and output move behind the adapters
	print, verbose := false, false
	var words []string
	ok := flagArgs(args, claudeValued, claudeBare, func(name, val string) bool {
		switch name {
		case "-p", "--print":
			print = true
		case "--model":
			r.model = val
		case "--output-format":
			r.format = val
			return val == "text" || val == "json" || val == "stream-json"
		case "--verbose":
			verbose = true
		case "--include-partial-messages":
			r.partial = true
		case "--permission-mode":
			r.mode = val
		case "--dangerously-skip-permissions":
			r.mode = "bypassPermissions"
		case "--effort":
			r.effort = val
		}
		return true
	}, func(w string) bool { words = append(words, w); return true })
	// stream-json wants --verbose (claude says so itself), and json with it
	// prints every message rather than the result.
	if !ok || !print || len(words) > 1 || r.format == "stream-json" && !verbose || r.format == "json" && verbose ||
		r.partial && r.format != "stream-json" {
		return workRun{}, false
	}
	if len(words) == 1 {
		r.prompt = words[0] // else it's on stdin
	}
	return r, true
}

// readStdin is what stdin holds when it isn't a terminal, as the programs
// read it; fed is whether it isn't. Claude Code gives up on it after a
// few seconds, and so does this: waited is whether it did.
func readStdin() (in []byte, fed, waited bool) {
	if term.IsTerminal(os.Stdin.Fd()) {
		return nil, false, false
	}
	got := make(chan []byte, 1)
	go func() { b, _ := io.ReadAll(io.LimitReader(os.Stdin, maxStdin)); got <- b }() // the rest is read by whoever runs
	select {
	case b := <-got:
		return b, true, false
	case <-time.After(3 * time.Second):
		return nil, true, true
	}
}

// realProgram is where prog is on path, past the stand-ins.
func realProgram(prog, path string) string {
	shims := filepath.Clean(host.ShimDir())
	if resolved, err := filepath.EvalSymlinks(shims); err == nil {
		shims = resolved
	}
	for _, d := range filepath.SplitList(host.WithoutShims(path)) {
		p := filepath.Join(d, prog)
		fi, err := os.Stat(p)
		if err != nil || fi.IsDir() || fi.Mode()&0o111 == 0 {
			continue
		}
		if r, err := filepath.EvalSymlinks(p); err == nil && filepath.Dir(r) == shims {
			continue
		}
		return p
	}
	if k, ok := agent.ProgramKind(prog); ok {
		if p := agent.Path(k); p != "" {
			resolved, err := filepath.EvalSymlinks(p)
			if err == nil && filepath.Dir(resolved) != shims {
				return p
			}
		}
	}
	return ""
}

// runReal runs the real prog in this process's place, with PATH as it
// was; fed, when set, is its stdin, from what was already read of it.
func runReal(prog string, args []string, path string, fed io.Reader) int {
	bin := realProgram(prog, path)
	if bin == "" {
		fmt.Fprintf(os.Stderr, "rush: can't find %s on PATH\n", prog)
		return 127
	}
	env := append(os.Environ(), "PATH="+path)
	if fed == nil {
		err := syscall.Exec(bin, append([]string{prog}, args...), env) // only returns if it failed
		fmt.Fprintln(os.Stderr, "rush:", err)
		return 126
	}
	cmd := exec.Command(bin, args...)
	cmd.Args[0] = prog
	cmd.Env, cmd.Stdin, cmd.Stdout, cmd.Stderr = env, fed, os.Stdout, os.Stderr
	signal.Ignore(syscall.SIGINT, syscall.SIGTERM, syscall.SIGHUP) // they reach it too, as its group's
	if err := cmd.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode()
		}
		fmt.Fprintln(os.Stderr, "rush:", err)
		return 126
	}
	return 0
}

// configDirEnv is where each agent's program is told its config folder.
var configDirEnv = map[agent.Kind]string{"claude": "CLAUDE_CONFIG_DIR", "codex": "CODEX_HOME"} // migration: per-agent CLI parsing and output move behind the adapters

// harness is the agent whose program k runs: its own, or the one a
// provider riding another's (Ollama in Claude Code) runs.
func harness(k agent.Kind) agent.Kind {
	if a, ok := agent.Get(k); ok {
		if rd, ok := a.(agent.Rider); ok {
			return rd.Rides()
		}
	}
	return k
}

// hostRun runs r as a rush session and prints it as its program would;
// false when it couldn't start one, and nothing was printed.
func hostRun(r workRun, stdout, stderr io.Writer) (int, bool) {
	cwd, err := os.Getwd()
	if err != nil {
		return 0, false
	}
	if r.cwd != "" {
		cwd = r.cwd
		if !filepath.IsAbs(cwd) {
			cwd = filepath.Join(must(os.Getwd()), cwd)
		}
	}
	base := harness(r.kind)
	if base == "codex" && !r.skipGit && !inRepo(cwd) {
		if r.kind != base { // the real codex would run it on OpenAI
			fmt.Fprintln(stderr, "Not inside a trusted directory and --skip-git-repo-check was not specified.")
			return 1, true
		}
		return 0, false // codex says why it won't run there
	}
	if r.mode == "" && r.kind == base {
		r.mode = parentMode(r.kind)
	}
	cfg := host.Config{Cwd: cwd, Model: r.model, Effort: r.effort, PermissionMode: r.mode, Prompt: r.prompt,
		Name: firstWordsOf(r.prompt), Meta: map[string]string{"spawnedBy": or(os.Getenv("RUSH_SESSION"), "shell")},
		Owner: os.Getpid()}
	if string(r.kind) == state.LoginsKind {
		cfg.Account = state.Load().Config.ActiveAccount().Profile()
	}
	if err := cfg.UseAgent(string(r.kind)); err != nil {
		return 0, false
	}
	// In the config folder the program would have used.
	if d := os.Getenv(configDirEnv[r.kind]); r.kind == base && d != "" && filepath.Clean(d) != filepath.Clean(cfg.Account.Dir) {
		cfg.Account.Dir = d
		if a, ok := agent.Get(r.kind); ok {
			for _, p := range a.Profiles() {
				if filepath.Clean(p.Dir) == filepath.Clean(d) {
					cfg.Account = p
				}
			}
		}
	}
	_ = host.SignInIfOut(string(r.kind), cfg.Account) // said once already, above
	request := outgoingExchange("", "request", r.prompt, nil)
	cfg.PromptExchange = request
	started, err := host.Spawn(cfg)
	if err != nil {
		return 0, false
	}
	if request != nil {
		request.Receiver = exchangePeer(started.ID)
		if err := mirrorOutgoing(request); err != nil {
			fmt.Fprintln(stderr, "rush: could not record delegated task:", err)
		}
	}
	dial := host.Dial
	if base == "claude" { // migration: per-agent CLI parsing and output move behind the adapters
		dial = host.DialRaw // its own lines, as claude -p prints them
	}
	c, err := dial(started.ID)
	if err != nil {
		if info, err := host.ReadInfo(started.ID); err == nil && info.HostPID > 0 {
			_ = syscall.Kill(info.HostPID, syscall.SIGTERM)
		}
		return 0, false
	}
	defer c.Close()
	// Stopped, it stops the session too, as the program would have ended.
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM, syscall.SIGHUP)
	var out printer
	switch base {
	case "codex":
		out = &codexOut{r: r, cwd: cwd, stdout: stdout, stderr: stderr}
	case "claude": // migration: per-agent CLI parsing and output move behind the adapters
		out = &claudeOut{r: r, stdout: stdout}
	default:
		out = &onceOut{stdout: stdout, stderr: stderr}
	}
	var captured *exchangePrinter
	if request != nil {
		captured = &exchangePrinter{printer: out}
		out = captured
	}
	code := follow(c, out, sig)
	if code == hostGone {
		// Why it went, as the agent that couldn't start said.
		info, _ := host.ReadInfo(started.ID)
		fmt.Fprintln(stderr, "rush:", or(info.Error, "the session's host went away"))
		code = 1
	}
	if captured != nil {
		recordReturn(request, started.ID, captured.text, code)
	}
	stopHost(c, started.ID)
	return code, true
}

// parentMode is the permission mode of the rush session whose shell this
// runs in, when it's the same agent's: a run it starts works for it, with
// its trust, not stuck on prompts no one sees. "" otherwise.
func parentMode(k agent.Kind) string {
	id := os.Getenv("RUSH_SESSION")
	if id == "" {
		return ""
	}
	cfg, err := host.ReadConfig(id)
	if err != nil || agent.Kind(or(cfg.Kind, string(agent.LegacyKind))) != k {
		return ""
	}
	info, err := host.ReadInfo(id)
	if err != nil {
		return ""
	}
	return info.PermissionMode
}

// stopHost ends the session's host, as the program would have ended: by
// asking, and if that doesn't take, by signal.
func stopHost(c *host.Client, id string) {
	_ = c.Stop()
	info, err := host.ReadInfo(id)
	if err != nil || info.HostPID <= 0 {
		return
	}
	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		if !host.Alive(info.HostPID) {
			return
		}
	}
	_ = syscall.Kill(info.HostPID, syscall.SIGTERM)
}

func must(s string, _ error) string { return s }

// inRepo is whether dir is in a git repository, which codex exec wants.
func inRepo(dir string) bool {
	for d := dir; ; d = filepath.Dir(d) {
		if _, err := os.Stat(filepath.Join(d, ".git")); err == nil {
			return true
		}
		if filepath.Dir(d) == d {
			return false
		}
	}
}

// printer prints a hosted run as its program would.
type printer interface {
	line(l []byte, ev any)
	finish() int
}

// hostGone is what follow returns when the session's host went away.
const hostGone = -1

// follow reads the session until it has answered and has nothing more
// to do: its turn over, and nothing you sent it while it ran left.
// Anything it asks is refused, as the program run this way would.
func follow(c *host.Client, out printer, sig <-chan os.Signal) int {
	ended := false
	for {
		select {
		case s := <-sig: // the session stops with it, after
			if s == syscall.SIGINT {
				return 130
			}
			return 143
		case l, ok := <-c.Lines:
			if !ok {
				return hostGone
			}
			ev, _ := host.Decode(l)
			out.line(l, ev)
			switch e := ev.(type) {
			case event.TurnEnd, headless.Result:
				ended = true
			case event.Approval:
				_ = c.Deny(e.ID, "not allowed in a run nobody is watching", false)
			case event.Question:
				_ = c.Deny(e.ID, "no one is there to answer", false)
			case headless.PermissionRequest:
				_ = c.Deny(e.ID, "Claude requested permissions to use "+e.Tool+", but you haven't granted it yet.", false)
			case host.InfoEvent:
				i := e.Info
				if i.State == "working" {
					ended = false
				}
				// Work it left in the background would be cut off: it
				// picks up again when that ends.
				busy := i.Retry != nil && !i.Retry.GaveUp || len(i.Background) > 0
				if ended && (i.State == "idle" && len(i.Queue) == 0 && !busy || i.Limit != nil) || i.State == "stopped" {
					return out.finish()
				}
			}
		}
	}
}

// claudeOut prints a hosted Claude Code run as claude -p does: the
// result's text, the result, or every line.
type claudeOut struct {
	r      workRun
	stdout io.Writer
	result []byte
	failed bool
}

// claudeOnly are the lines of Claude Code's a hosted session has and
// claude -p doesn't print.
var claudeOnly = map[string]bool{"control_request": true, "control_response": true, "control_cancel_request": true, "keep_alive": true}

func (o *claudeOut) line(l []byte, _ any) {
	var head struct {
		Type    string `json:"type"`
		Subtype string `json:"subtype"`
		Sent    bool   `json:"agtop_sent"`
		IsError bool   `json:"is_error"`
	}
	if jsonx.Unmarshal(l, &head) != nil || head.Sent || strings.HasPrefix(head.Type, "agtop_") || claudeOnly[head.Type] {
		return
	}
	// Streamed deltas, and the status that comes with them, are only
	// printed when asked for.
	if (head.Type == "stream_event" || head.Type == "system" && head.Subtype == "status") && !o.r.partial {
		return
	}
	if head.Type == "result" {
		o.result, o.failed = append(o.result[:0], l...), head.IsError
	}
	if o.r.format == "stream-json" {
		_, _ = o.stdout.Write(append(bytes.Clone(l), '\n'))
	}
}

func (o *claudeOut) finish() int {
	switch {
	case o.result == nil:
		return 1
	case o.r.format == "json":
		_, _ = o.stdout.Write(append(o.result, '\n'))
	case o.r.format == "text":
		var res struct {
			Result string `json:"result"`
		}
		_ = jsonx.Unmarshal(o.result, &res)
		fmt.Fprintln(o.stdout, res.Result)
	}
	if o.failed {
		return 1
	}
	return 0
}

// onceOut prints a hosted one-shot run as the programs rush reads with
// agent.OnceReader do: the last answer on stdout, or why it failed on
// stderr.
type onceOut struct {
	stdout, stderr io.Writer
	last, err      string
}

func (o *onceOut) line(_ []byte, ev any) {
	switch e := ev.(type) {
	case event.Message:
		var text string
		for _, p := range e.Parts {
			if p.Kind == event.Text {
				text += p.Text
			}
		}
		if e.Role == "assistant" && text != "" {
			o.last = text
		}
	case event.TurnEnd:
		if o.err = ""; e.Reason == "error" {
			o.err = or(e.Err, "turn failed")
		}
	}
}

func (o *onceOut) finish() int {
	if o.err != "" {
		fmt.Fprintln(o.stderr, "Error:", o.err)
		return 1
	}
	if o.last != "" {
		fmt.Fprintln(o.stdout, o.last)
	}
	return 0
}

// codexOut prints a hosted Codex run as codex exec does: progress on
// stderr and the last message on stdout, or its events as JSON lines.
type codexOut struct {
	r              workRun
	cwd            string
	stdout, stderr io.Writer
	ids            map[string]string // Codex's item ids as exec numbers them
	last           string            // the turn's last message
	err            string            // why the last turn failed
	turn           bool
	plan           string      // the todo list's item, this turn
	todo           *event.Plan // and what it last said
	tokens         int64
}

func (o *codexOut) id(of string) string {
	if o.ids == nil {
		o.ids = map[string]string{}
	}
	if id, ok := o.ids[of]; ok {
		return id
	}
	o.ids[of] = fmt.Sprintf("item_%d", len(o.ids))
	return o.ids[of]
}

func (o *codexOut) emit(v any) {
	if b, err := jsonx.Marshal(v); err == nil {
		_, _ = o.stdout.Write(append(b, '\n'))
	}
}

func (o *codexOut) line(_ []byte, ev any) {
	switch e := ev.(type) {
	case event.Init:
		if o.r.json {
			o.emit(struct {
				Type   string `json:"type"`
				Thread string `json:"thread_id"`
			}{"thread.started", e.SessionID})
			return
		}
		fmt.Fprintf(o.stderr, "OpenAI Codex v%s\n--------\nworkdir: %s\nmodel: %s\n", strings.TrimPrefix(e.Version, "v"), or(e.Cwd, o.cwd), e.Model)
		if o.r.effort != "" {
			fmt.Fprintf(o.stderr, "reasoning effort: %s\n", o.r.effort)
		}
		fmt.Fprintf(o.stderr, "session id: %s\n--------\nuser\n%s\n", e.SessionID, o.r.prompt)
	case host.Sent:
		if !o.r.json && e.Text != o.r.prompt {
			fmt.Fprintf(o.stderr, "user\n%s\n", e.Text)
		}
	case event.Status:
		if e.Busy && e.Text == "" && !o.turn {
			o.turn, o.last, o.err, o.plan, o.todo = true, "", "", "", nil
			if o.r.json {
				o.emit(map[string]string{"type": "turn.started"})
			}
		}
	case event.Message:
		for _, p := range e.Parts {
			o.part(e, p)
		}
	case event.Plan:
		o.todo = &e
		o.todos("")
	case event.TurnEnd:
		o.turnEnd(e)
	}
}

func (o *codexOut) part(m event.Message, p event.Part) {
	switch {
	case m.Role == "assistant" && p.Kind == event.Text:
		o.last = p.Text
		if o.r.json {
			o.emit(itemLine{"item.completed", execText{ID: o.id(m.ID), Type: "agent_message", Text: p.Text}})
			return
		}
		fmt.Fprintf(o.stderr, "codex\n%s\n", p.Text)
	case m.Role == "assistant" && p.Kind == event.Thinking:
		if o.r.json {
			o.emit(itemLine{"item.completed", execText{ID: o.id(m.ID), Type: "reasoning", Text: p.Text}})
			return
		}
		fmt.Fprintf(o.stderr, "thinking\n%s\n", p.Text)
	case p.Kind == event.ToolCall && p.Call != nil:
		if o.r.json {
			if it, ok := execItem(o.id(p.Call.ID), p.Call.Raw); ok {
				o.emit(itemLine{"item.started", it})
			}
			return
		}
		if p.Call.Input.Command != "" {
			fmt.Fprintf(o.stderr, "exec\n%s in %s\n", p.Call.Input.Command, or(p.Call.Input.Cwd, o.cwd))
		}
	case p.Kind == event.ToolResult && p.Output != nil:
		if o.r.json {
			if it, ok := execItem(o.id(p.Output.CallID), p.Output.Raw); ok {
				o.emit(itemLine{"item.completed", it})
			}
			return
		}
		if p.Output.Exit != nil {
			if *p.Output.Exit == 0 {
				fmt.Fprintf(o.stderr, " succeeded:\n%s\n", p.Output.Text)
			} else {
				fmt.Fprintf(o.stderr, " exited %d:\n%s\n", *p.Output.Exit, p.Output.Text)
			}
		}
	}
}

// todos is the turn's plan as exec's todo_list item: started, then
// updated, and completed as the turn ends (how, then).
func (o *codexOut) todos(how string) {
	if !o.r.json || o.todo == nil {
		return
	}
	kind := "item.updated"
	if o.plan == "" {
		o.plan, kind = o.id(fmt.Sprintf("plan:%d", len(o.ids))), "item.started"
	}
	if how != "" {
		kind = how
	}
	type todo struct {
		Text      string `json:"text"`
		Completed bool   `json:"completed"`
	}
	items := []todo{}
	for _, t := range o.todo.Todos {
		items = append(items, todo{t.Label, t.Status == "completed"})
	}
	o.emit(itemLine{kind, struct {
		ID    string `json:"id"`
		Type  string `json:"type"`
		Items []todo `json:"items"`
	}{o.plan, "todo_list", items}})
}

func (o *codexOut) turnEnd(e event.TurnEnd) {
	o.todos("item.completed")
	o.turn, o.todo = false, nil
	o.tokens += e.Tokens.Input + e.Tokens.Output
	if e.Reason == "error" {
		o.err = or(e.Err, "turn failed")
	}
	if !o.r.json {
		if o.err != "" {
			fmt.Fprintf(o.stderr, "ERROR: %s\n", o.err)
		}
		return
	}
	if o.err != "" {
		o.emit(struct {
			Type  string            `json:"type"`
			Error map[string]string `json:"error"`
		}{"turn.failed", map[string]string{"message": o.err}})
		return
	}
	t := e.Tokens
	o.emit(map[string]any{"type": "turn.completed", "usage": struct {
		Input      int64 `json:"input_tokens"`
		Cached     int64 `json:"cached_input_tokens"`
		CacheWrite int64 `json:"cache_write_input_tokens"`
		Output     int64 `json:"output_tokens"`
		Reasoning  int64 `json:"reasoning_output_tokens"`
	}{t.Input + t.CacheRead, t.CacheRead, t.CacheWrite5m + t.CacheWrite1h, t.Output, t.Reasoning}})
}

func (o *codexOut) finish() int {
	if !o.r.json {
		fmt.Fprintf(o.stderr, "tokens used\n%d\n", o.tokens)
		if o.last != "" {
			fmt.Fprintln(o.stdout, o.last)
		}
	}
	if o.err != "" {
		return 1 // and -o is left as it was, as exec leaves it
	}
	if o.r.lastMsg != "" {
		_ = os.WriteFile(o.r.lastMsg, []byte(o.last), 0o644)
	}
	return 0
}

// itemLine is one of exec's item events.
type itemLine struct {
	Type string `json:"type"`
	Item any    `json:"item"`
}

// execText is exec's agent_message and reasoning items.
type execText struct {
	ID   string `json:"id"`
	Type string `json:"type"`
	Text string `json:"text"`
}

// execItem is one of Codex's app-server items as exec prints it, by the
// id exec gives it; false for the kinds exec doesn't print.
func execItem(id string, raw jsontext.Value) (any, bool) {
	var it struct {
		Type             string         `json:"type"`
		Command          string         `json:"command"`
		AggregatedOutput *string        `json:"aggregatedOutput"`
		ExitCode         *int           `json:"exitCode"`
		Status           string         `json:"status"`
		Server           string         `json:"server"`
		Tool             string         `json:"tool"`
		Arguments        jsontext.Value `json:"arguments"`
		Result           *struct {
			Content           jsontext.Value `json:"content"`
			StructuredContent jsontext.Value `json:"structuredContent"`
		} `json:"result"`
		Error *struct {
			Message string `json:"message"`
		} `json:"error"`
		Query   string         `json:"query"`
		Action  jsontext.Value `json:"action"`
		Changes []struct {
			Path string `json:"path"`
			Kind struct {
				Type string `json:"type"`
			} `json:"kind"`
		} `json:"changes"`
	}
	if len(raw) == 0 || jsonx.Unmarshal(raw, &it) != nil {
		return nil, false
	}
	status := strings.ToLower(strings.ReplaceAll(it.Status, "inProgress", "in_progress"))
	switch it.Type {
	case "commandExecution":
		out := ""
		if it.AggregatedOutput != nil {
			out = *it.AggregatedOutput
		}
		return struct {
			ID       string `json:"id"`
			Type     string `json:"type"`
			Command  string `json:"command"`
			Output   string `json:"aggregated_output"`
			ExitCode *int   `json:"exit_code"`
			Status   string `json:"status"`
		}{id, "command_execution", it.Command, out, it.ExitCode, status}, true
	case "fileChange":
		type change struct {
			Path string `json:"path"`
			Kind string `json:"kind"`
		}
		changes := []change{}
		for _, c := range it.Changes {
			changes = append(changes, change{c.Path, c.Kind.Type})
		}
		return struct {
			ID      string   `json:"id"`
			Type    string   `json:"type"`
			Changes []change `json:"changes"`
			Status  string   `json:"status"`
		}{id, "file_change", changes, status}, true
	case "mcpToolCall":
		type result struct {
			Content    jsontext.Value `json:"content"`
			Structured jsontext.Value `json:"structured_content"`
		}
		var res *result
		if it.Result != nil {
			res = &result{it.Result.Content, it.Result.StructuredContent}
		}
		args := it.Arguments
		if len(args) == 0 {
			args = jsontext.Value("null")
		}
		return struct {
			ID     string         `json:"id"`
			Type   string         `json:"type"`
			Server string         `json:"server"`
			Tool   string         `json:"tool"`
			Args   jsontext.Value `json:"arguments"`
			Result *result        `json:"result"`
			Error  any            `json:"error"`
			Status string         `json:"status"`
		}{id, "mcp_tool_call", it.Server, it.Tool, args, res, it.Error, status}, true
	case "webSearch":
		action := it.Action
		if len(action) == 0 {
			action = jsontext.Value("null")
		}
		return struct {
			ID     string         `json:"id"`
			Type   string         `json:"type"`
			Query  string         `json:"query"`
			Action jsontext.Value `json:"action"`
		}{id, "web_search", it.Query, action}, true
	}
	return nil, false
}
