package convo

import (
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agtools"
)

// Spawn is another agent a shell command ran: claude -p, codex exec,
// copilot -p. Read from the command, it's only a hint: whoever draws the
// conversation finds the sessions it ran and hands them to SetChildren.
type Spawn struct {
	Kind   agent.Kind
	Name   string // what the agent is called: Codex, Claude Code
	Prompt string // what it was asked; empty when it came from a file or a pipe
	From   string // the file its prompt was fed from, when it was
	Model  string
	Dir    string // where it ran, when the command went somewhere first
	// Child is the run a harness's own subagent call names, when its
	// harness keeps it apart (agent.ChildFinder); Kind is then the session's.
	Child string
}

// spawnNot are the first words that make an agent's program do something
// other than work: set up, sign in, list, report.
var spawnNot = map[string]bool{
	"mcp": true, "config": true, "doctor": true, "update": true, "upgrade": true, "install": true, "login": true,
	"logout": true, "auth": true, "plugin": true, "plugins": true, "setup-token": true, "migrate-installer": true,
	"completion": true, "features": true, "apply": true, "app-server": true, "mcp-server": true, "sandbox": true,
	"debug": true, "cloud": true, "models": true, "version": true, "help": true, "agents": true, "acp": true,
	"--version": true, "-v": true, "-V": true, "--help": true, "-h": true, "--acp": true,
}

// spawnSub are the first words that start a run and name nothing else:
// codex exec, opencode run.
var spawnSub = map[string]bool{"exec": true, "e": true, "run": true}

// spawnValued are the flags that take a value, which isn't the prompt.
var spawnValued = map[string]bool{
	"--model": true, "-m": true, "--output-format": true, "--input-format": true, "--append-system-prompt": true,
	"--system-prompt": true, "--allowedTools": true, "--allowed-tools": true, "--disallowedTools": true,
	"--disallowed-tools": true, "--permission-mode": true, "--add-dir": true, "--max-turns": true, "--resume": true,
	"-r": true, "--session-id": true, "--settings": true, "--mcp-config": true, "--agents": true, "--agent": true,
	"--fallback-model": true, "--effort": true, "-C": true, "--cd": true, "-c": true, "--config": true, "-s": true,
	"--sandbox": true, "-i": true, "--image": true, "--profile": true, "-a": true, "--ask-for-approval": true,
	"--output-last-message": true, "-o": true, "--color": true, "--output-schema": true, "--log-level": true,
	"--allow-tool": true, "--deny-tool": true, "--betas": true, "--setting-sources": true, "--plugin-dir": true,
	"--json-schema": true, "--max-budget-usd": true, "--permission-prompt-tool": true, "--permission-prompts": true,
	"--name": true, "-n": true, "--reasoning-effort": true, "--agent-file": true, "--workdir": true, "--dir": true,
}

// spawnPrint are the flags that run the agent once and print: the prompt
// follows them for most agents (claude's -p takes none, so its prompt is
// the next word either way).
var spawnPrint = map[string]bool{"-p": true, "--print": true, "--prompt": true}

// SpawnOf is the agent cmd runs, when it runs one: a registered agent's
// program, asked to work rather than to set up or report.
func SpawnOf(cmd string) (Spawn, bool) {
	segs := segments(strings.ReplaceAll(strings.TrimSpace(cmd), "\\\n", " "))
	dir := ""
	for _, s := range segs {
		c := shellCmd(s.text)
		if len(c.words) >= 2 && c.words[0] == "cd" {
			dir = unquoteArg(c.words[1])
		}
		if sp, ok := spawnIn(c, s.body, ""); ok {
			return withDir(sp, dir), true
		}
		// Fed a prompt through a pipe: echo "…" | claude -p.
		for _, f := range s.filters {
			if sp, ok := spawnIn(shellCmd(f), nil, piped(c.words)); ok {
				if sp.Prompt == "" && sp.From == "" && len(c.words) >= 2 && filepath.Base(c.words[0]) == "cat" {
					sp.From = unquoteArg(c.words[1])
				}
				return withDir(sp, dir), true
			}
		}
	}
	return Spawn{}, false
}

// command is one simple command as the shell reads it: its words (quotes
// kept, a word running on across them), and what it's fed on stdin.
type command struct {
	words []string
	stdin string // a file fed with <
	tag   string // a heredoc fed with <<
}

// shellCmd reads the first simple command of s: up to an unquoted ;, &,
// | or newline, its redirects taken out.
func shellCmd(s string) command {
	var c command
	var cur strings.Builder
	in := false // a word has begun
	redir := "" // the redirect whose target comes next
	flush := func() {
		if !in {
			return
		}
		w := cur.String()
		cur.Reset()
		in = false
		switch redir {
		case "<":
			c.stdin = unquoteArg(w)
		case "<<":
			c.tag = strings.Trim(w, `'"`)
		case "":
			c.words = append(c.words, w)
		}
		redir = ""
	}
	for i := 0; i < len(s); i++ {
		ch := s[i]
		switch {
		case ch == '\\' && i+1 < len(s):
			cur.WriteByte(ch)
			cur.WriteByte(s[i+1])
			i++
			in = true
		case ch == '\'':
			j := strings.IndexByte(s[i+1:], '\'')
			if j < 0 {
				j = len(s) - i - 1
			}
			cur.WriteString(s[i:min(len(s), i+j+2)])
			i += j + 1
			in = true
		case ch == '"':
			j := closeQuote(s, i)
			cur.WriteString(s[i:j])
			i = j - 1
			in = true
		case ch == '$' && i+1 < len(s) && s[i+1] == '(':
			j := closeParen(s, i+1)
			cur.WriteString(s[i:j])
			i = j - 1
			in = true
		case ch == ' ' || ch == '\t':
			flush()
		case ch == ';' || ch == '|' || ch == '\n' || ch == '&' && !(i+1 < len(s) && s[i+1] == '>'):
			flush()
			return c
		case ch == '<' || ch == '>' || ch == '&':
			flush()
			op := string(ch)
			for i+1 < len(s) && strings.IndexByte("<>&", s[i+1]) >= 0 {
				i++
				op += string(s[i])
			}
			// 2>&1 and >&2 are whole in themselves.
			if strings.HasSuffix(op, "&") && i+1 < len(s) && s[i+1] >= '0' && s[i+1] <= '9' {
				i++
				redir = ""
				continue
			}
			redir = op
			if op != "<" && op != "<<" && op != "<<-" {
				redir = ">"
			} else if op == "<<-" {
				redir = "<<"
			}
			// What's written to is no word of the command.
			in = false
		case ch >= '0' && ch <= '9' && !in && i+1 < len(s) && s[i+1] == '>':
			// the 2 of 2>
		default:
			cur.WriteByte(ch)
			in = true
		}
	}
	flush()
	return c
}

// closeQuote is just past the " that closes the one at i, past any $(…)
// inside it and its own quotes.
func closeQuote(s string, i int) int {
	for j := i + 1; j < len(s); j++ {
		switch {
		case s[j] == '\\':
			j++
		case s[j] == '$' && j+1 < len(s) && s[j+1] == '(':
			j = closeParen(s, j+1) - 1
		case s[j] == '"':
			return j + 1
		}
	}
	return len(s)
}

// closeParen is just past the ) that closes the ( at i.
func closeParen(s string, i int) int {
	depth := 0
	for j := i; j < len(s); j++ {
		switch s[j] {
		case '\\':
			j++
		case '\'':
			if k := strings.IndexByte(s[j+1:], '\''); k >= 0 {
				j += k + 1
			}
		case '"':
			j = closeQuote(s, j) - 1
		case '(':
			depth++
		case ')':
			if depth--; depth == 0 {
				return j + 1
			}
		}
	}
	return len(s)
}

func withDir(sp Spawn, dir string) Spawn {
	if sp.Dir == "" {
		sp.Dir = dir
	}
	return sp
}

// piped is what echo or printf writes into a pipe: a prompt given that way.
func piped(words []string) string {
	if len(words) >= 2 && (words[0] == "echo" || words[0] == "printf") {
		for _, w := range words[1:] {
			if !strings.HasPrefix(w, "-") {
				return unquoteArg(w)
			}
		}
	}
	return ""
}

// catRe is a prompt read from a file as it's given: "$(cat prompt.md)".
var catRe = regexp.MustCompile(`^"?\$\((?:cat|<)\s+("[^"]+"|'[^']+'|[^\s)]+)\s*\)"?$`)

// spawnIn reads one command as an agent run: its program (past any runner
// such as timeout or npx), and then its prompt and model. stdin is what
// a pipe fed it, and body the lines after its own, for a heredoc.
func spawnIn(c command, body []string, stdin string) (Spawn, bool) {
	words := c.words
	for len(words) > 0 && assignRe.MatchString(words[0]) {
		words = words[1:]
	}
	for len(words) > 1 {
		switch w := filepath.Base(words[0]); w {
		case "timeout", "gtimeout":
			words = words[1:]
			for len(words) > 1 && strings.HasPrefix(words[0], "-") {
				words = words[1:]
			}
			if len(words) > 1 {
				words = words[1:] // the duration
			}
			continue
		case "env", "nohup", "time", "exec", "command", "caffeinate", "npx", "bunx", "pnpx":
			words = words[1:]
			for len(words) > 1 && (strings.HasPrefix(words[0], "-") || assignRe.MatchString(words[0])) {
				words = words[1:]
			}
			continue
		}
		break
	}
	if len(words) == 0 {
		return Spawn{}, false
	}
	if len(words) >= 3 && filepath.Base(words[0]) == "rush" && words[1] == "session" && words[2] == "start" {
		return rushStart(words[3:], c, body, stdin)
	}
	k, ok := agent.ProgramKind(unquoteArg(words[0]))
	if !ok {
		return Spawn{}, false
	}
	sp := Spawn{Kind: k, Name: string(k)}
	if a, ok := agent.Get(k); ok {
		sp.Name = a.Name()
	}
	args := words[1:]
	print, sub, fromStdin, streamed := false, false, false, false
	var loose []string // bare words after a flag rush doesn't know, which may be its value
	for i := 0; i < len(args); i++ {
		a := args[i]
		// The subcommand is the first bare word, past any global flags:
		// codex --search exec.
		first := !sub && !print && sp.Prompt == "" && sp.From == "" && len(loose) == 0
		switch {
		case a == "--help" || a == "-h" || a == "--version":
			return Spawn{}, false // it reports, wherever it's asked to
		case spawnNot[a] && sp.Prompt == "" && !sub:
			return Spawn{}, false
		case spawnSub[a] && first:
			sub = true
		case spawnPrint[a]:
			print = true
		case a == "-":
			fromStdin = true
		case strings.HasPrefix(a, "-"):
			name, val, eq := strings.Cut(a, "=")
			next := ""
			if i+1 < len(args) {
				next = args[i+1]
			}
			if eq {
				next = val
			}
			switch name {
			case "--model", "-m":
				sp.Model = unquoteArg(next)
			case "-C", "--cd", "--workdir", "--dir":
				sp.Dir = unquoteArg(next)
			case "--input-format":
				streamed = unquoteArg(next) == "stream-json"
			}
			switch {
			case eq:
			case spawnValued[name]:
				i++
			case strings.HasPrefix(name, "--") && next != "" && !strings.HasPrefix(next, "-") && !quoted(next) && (!first || !spawnSub[next]):
				loose = append(loose, next)
				i++
			}
		case sp.Prompt == "" && sp.From == "":
			if m := catRe.FindStringSubmatch(a); m != nil {
				sp.From = unquoteArg(m[1])
			} else {
				sp.Prompt = promptOf(a, body)
			}
		}
	}
	// A bare word after a flag rush doesn't know is the prompt only when
	// nothing else is.
	if sp.Prompt == "" && sp.From == "" && len(loose) > 0 && !fromStdin && c.stdin == "" && c.tag == "" && stdin == "" {
		sp.Prompt = unquoteArg(loose[len(loose)-1])
	}
	if sp.Prompt == "" && sp.From == "" {
		switch {
		case stdin != "":
			sp.Prompt = stdin
		case c.tag != "":
			sp.Prompt = strings.TrimSpace(strings.Join(heredoc(body, c.tag), "\n"))
		case c.stdin != "" && c.stdin != "/dev/null":
			sp.From = c.stdin
		}
	}
	// Messages as JSON aren't a prompt to show.
	if streamed {
		sp.Prompt = ""
	}
	// A bare program with nothing asked of it opens its own screen, and
	// codex needs exec to run without one.
	if sp.Kind == "codex" && !sub {
		return Spawn{}, false
	}
	if !print && !sub && sp.Prompt == "" && sp.From == "" {
		return Spawn{}, false
	}
	return sp, true
}

// rushStart reads rush session start's args as the agent it starts: a
// session of its own, which rush lists as its starter's subagent.
func rushStart(args []string, c command, body []string, stdin string) (Spawn, bool) {
	sp := Spawn{Kind: agent.LegacyKind}
	if slices.Contains(args, "--resume") {
		return Spawn{}, false // one it had already
	}
	for i := 0; i+1 < len(args); i++ {
		name, val, eq := strings.Cut(args[i], "=")
		if !eq {
			val = args[i+1]
		}
		switch v := unquoteArg(val); name {
		case "--agent":
			sp.Kind = agent.Kind(v)
		case "--model":
			sp.Model = v
		case "--cwd":
			sp.Dir = v
		case "--prompt-file":
			sp.From = v
		}
	}
	if sp.From == "-" {
		sp.From = ""
		switch {
		case stdin != "":
			sp.Prompt = stdin
		case c.tag != "":
			sp.Prompt = strings.TrimSpace(strings.Join(heredoc(body, c.tag), "\n"))
		case c.stdin != "":
			sp.From = c.stdin
		}
	}
	sp.Name = string(sp.Kind)
	if a, ok := agent.Get(sp.Kind); ok {
		sp.Name = a.Name()
	}
	return sp, true
}

// toolRun is the agent st's call of rush's spawn_agent tool starts, as
// its input says, read once per input; nil for any other step.
func (st *Step) toolRun() *Spawn {
	if !agtools.IsSpawn(st.Tool) {
		return nil
	}
	if st.toolAt != len(st.Input)+1 {
		st.toolAt = len(st.Input) + 1
		st.toolSp = nil
		if in, ok := agtools.ReadSpawn(st.Input); ok {
			st.toolSp = &Spawn{Name: in.Agent, Prompt: in.Prompt, Model: in.Model}
		}
	}
	return st.toolSp
}

// agentRun is the agent st is drawn as: the one it was found to run, else
// the one its spawn_agent call names.
func (st *Step) agentRun() *Spawn {
	if st.run != nil {
		return st.run
	}
	return st.toolRun()
}

func quoted(w string) bool { return strings.HasPrefix(w, `"`) || strings.HasPrefix(w, "'") }

// promptOf is a prompt as written: quoted, or fed in by $(cat <<'EOF' …).
func promptOf(w string, body []string) string {
	if m := heredocArg.FindStringSubmatch(w); m != nil {
		// Quoted, the heredoc is part of the word; bare, it's the lines
		// that follow the command's.
		if v, ok := argValue(w); ok && strings.Contains(w, "\n") {
			return strings.TrimSpace(v)
		}
		return strings.TrimSpace(strings.Join(heredoc(body, m[1]), "\n"))
	}
	return unquoteArg(w)
}

// heredoc is a heredoc's lines, up to its tag.
func heredoc(body []string, tag string) []string {
	for i, l := range body {
		if strings.TrimSpace(l) == tag || strings.HasPrefix(strings.TrimSpace(l), tag+")") {
			return body[:i]
		}
	}
	return body
}

// unquoteArg is a word as the program gets it, escapes and all.
func unquoteArg(w string) string {
	if len(w) >= 2 && (w[0] == '"' || w[0] == '\'') {
		if v, ok := argValue(w); ok {
			return v
		}
	}
	return w
}

// Spawn is the agent the step's command ran, when it ran one, or the
// subagent it started when that keeps a session of its own.
func (st *Step) Spawn() (Spawn, bool) {
	if sp := st.toolRun(); sp != nil {
		return *sp, true
	}
	if st.kind() == tool.Subagent {
		in := st.in()
		if in.Child == "" {
			return Spawn{}, false
		}
		return Spawn{Name: agentName(st), Prompt: firstNonEmpty(in.Prompt, in.Description), Child: in.Child}, true
	}
	if st.kind() != tool.Shell {
		return Spawn{}, false
	}
	if st.spawnAt != len(st.Input)+1 {
		st.spawnAt = len(st.Input) + 1
		st.spawn = nil
		if sp, ok := SpawnOf(st.in().Command); ok {
			st.spawn = &sp
		}
	}
	if st.spawn == nil {
		return Spawn{}, false
	}
	return *st.spawn, true
}

// Spawns are the steps that started an agent whose session is its own, in
// order: from the shell, or as a harness's subagent kept apart.
func (s *Session) Spawns() []*Step {
	var out []*Step
	for _, t := range s.Turns {
		for _, it := range t.Items {
			if it.Kind == KStep && it.Step != nil {
				if _, ok := it.Step.Spawn(); ok {
					out = append(out, it.Step)
				}
			}
		}
	}
	return out
}

// Window is when a shell step could have started an agent: from its start
// to its end, or its background task's, and a grace after.
type Window struct {
	Step     string
	Command  string
	From, To time.Time
	// Asked is what a spawn_agent call asked: the agent it started was
	// asked exactly that.
	Asked string
}

// Windows are every shell step's and spawn_agent call's, in order; one
// still running (or its task) runs to now.
func (s *Session) Windows(now time.Time, grace time.Duration) []Window {
	var out []Window
	for _, t := range s.Turns {
		for _, it := range t.Items {
			st := it.Step
			if it.Kind != KStep || st == nil || st.kind() != tool.Shell && st.toolRun() == nil || st.Start.IsZero() {
				continue
			}
			end := st.End
			if j := s.jobOf(st.ID); j != nil && j.Background {
				end = j.End
			}
			if st.Status == Running || end.IsZero() || s.JobRunning(st.ID) {
				end = now
			}
			w := Window{Step: st.ID, Command: st.in().Command, From: st.Start, To: end.Add(grace)}
			if sp := st.toolRun(); sp != nil {
				w.Asked = sp.Prompt
			}
			out = append(out, w)
		}
	}
	return out
}

func (s *Session) jobOf(stepID string) *Job {
	if js := s.callJobs(stepID); len(js) > 0 {
		return js[0]
	}
	return nil
}

// Child is an agent a step ran whose session is its own: found by the
// session rush hosted it as, or by its transcript.
type Child struct {
	ID    string
	Spawn Spawn
	Sess  *Session
	Live  bool
	Start time.Time
}

// SetChildren shows kids as the agents st ran, as subagents are drawn: one
// is st's own row, several a row each under it. Call it again as they
// change; with none, st draws as the command it is.
func (s *Session) SetChildren(st *Step, kids []Child) {
	st.run, st.fan, st.runLive, st.runEnd, st.child, st.Children = nil, false, false, time.Time{}, nil, nil
	if len(kids) == 1 {
		k := kids[0]
		sp := k.Spawn
		if hint, ok := st.Spawn(); ok {
			if sp.Name == "" {
				sp.Kind, sp.Name = hint.Kind, hint.Name
			}
			sp.Prompt, sp.From = firstNonEmpty(sp.Prompt, hint.Prompt), firstNonEmpty(sp.From, hint.From)
			sp.Model, sp.Dir = firstNonEmpty(sp.Model, hint.Model), firstNonEmpty(sp.Dir, hint.Dir)
		}
		st.run, st.child, st.Children, st.runLive = &sp, k.Sess, shownSteps(k.Sess), k.Live
		if !k.Live {
			st.runEnd = k.Sess.Last
		}
	}
	if len(kids) > 1 {
		st.fan = true
		for _, k := range kids {
			sp := k.Spawn
			c := &Step{ID: st.ID + "/" + k.ID, Tool: st.Tool, Kind: tool.Shell, Status: OK, Start: k.Start, run: &sp, child: k.Sess, Children: shownSteps(k.Sess)}
			if k.Live {
				c.Status, st.runLive = Running, true
			} else if c.End = k.Sess.Last; c.End.After(st.runEnd) {
				st.runEnd = c.End
			}
			st.Children = append(st.Children, c)
		}
	}
	st.ranVer++
	s.touchStep(st)
	// A turn its task woke is the agents' reply, now they're known.
	for _, t := range s.Turns {
		if t.wokeBy != nil && t.wokeBy.ToolUseID == st.ID {
			s.wake(t)
		}
	}
}

// shownSteps are a session's steps as a subagent's are drawn.
func shownSteps(sess *Session) []*Step {
	var kids []*Step
	for _, t := range sess.Turns {
		for _, it := range t.Items {
			if it.Kind == KStep && it.Step != nil && !hidden(it.Step) {
				kids = append(kids, it.Step)
			}
		}
	}
	return kids
}

// agentSteps are the steps the agents t's commands ran took, counted with
// its own as a subagent's are.
func (t *Turn) agentSteps() int {
	n := 0
	for _, it := range t.Items {
		if st := it.Step; it.Kind == KStep && st != nil && st.run != nil {
			n += len(st.Children)
		} else if st != nil && st.fan {
			for _, c := range st.Children {
				n += len(c.Children)
			}
		}
	}
	return n
}

// ranAgents is whether st is drawn as the agents it ran: found, not only
// read from its command.
func (st *Step) ranAgents() bool { return st.run != nil || st.fan }

// spawnShown is how many of a running spawned agent's steps show under
// its row.
const spawnShown = 4

// spawnLabel is a spawned agent's row, as any subagent's: ⇉, the agent's
// name, then what the command says it's for (or what it was asked).
func spawnLabel(sp Spawn, desc string, lbl func(string) string) string {
	return glyphColor("⇉") + " " + bold + lbl(sp.Name) + "  " + faint(firstNonEmpty(desc, asked(sp)))
}

// asked is what a spawned agent was asked, in a line.
func asked(sp Spawn) string {
	if what := oneLine(sp.Prompt); what != "" || sp.From == "" {
		return what
	}
	return "prompt from " + filepath.Base(sp.From)
}

// agentsAsked is what the agents st ran were asked, in a line.
func (st *Step) agentsAsked() string {
	if st.run != nil {
		return asked(*st.run)
	}
	return plural(len(st.Children), "agent")
}

// reply is what a subagent said back, drawn as its opened row's body: the
// last answer of its own session once that's found, else what its call
// returned (a spawned agent's stdout, or its stderr when that's all). One
// at work has said nothing back yet, whatever its shell printed.
func reply(st *Step) string {
	if st.child != nil {
		if a := st.child.LastAnswer(1); a != "" || st.runLive || st.Status == Running {
			return a
		}
	}
	o := st.out()
	return firstNonEmpty(o.Stdout, o.Stderr, st.Output)
}

// agentNames are who the agents st ran are, each named once.
func (st *Step) agentNames() string {
	if st.run != nil {
		return st.run.Name
	}
	var names []string
	for _, c := range st.Children {
		if c.run != nil && !slices.Contains(names, c.run.Name) {
			names = append(names, c.run.Name)
		}
	}
	return strings.Join(names, ", ")
}

// wokeAgents is the step that ran the agents whose task woke the turn, once
// they're found: the turn is their reply.
func (d *drawer) wokeAgents() *Step {
	if j := d.t.wokeBy; j != nil {
		if a := d.s.byID[j.ToolUseID]; a != nil && a.ranAgents() {
			return a
		}
	}
	return nil
}

// replyOf is whether st reads the output of the task that woke its turn,
// when that ran agents: their reply, which the turn shows as theirs.
func (d *drawer) replyOf(st *Step) bool {
	if st.kind() != tool.Read || d.wokeAgents() == nil {
		return false
	}
	j, p := d.t.wokeBy, st.in().Path
	return p != "" && (p == j.OutputFile || filepath.Base(p) == j.ID+".output")
}

// replyRows is how much of a reply its card shows until it's opened.
const replyRows = 8

// replies draws what the agents a step ran said back, a card each, as a
// subagent's report is drawn: what it was asked on the top edge (the
// turn's header names who), its reply laid out as Markdown inside.
func (d *drawer) replies(a *Step, ref string, indent int) {
	kids := []*Step{a}
	if a.fan {
		kids = a.Children
	}
	room := min(min(d.cw, capRow)-indent-4, 72)
	for _, k := range kids {
		if k.run == nil {
			continue
		}
		head := paint(cBlue, "↩") + " " + faint(firstNonEmpty(asked(*k.run), k.run.Name))
		if room < 24 {
			d.add(ref, "", d.spine()+blanks(indent-1)+head, "")
			continue
		}
		// Laid out at the card's width, then taken back to go inside it.
		n, cw := len(d.lines), d.cw
		d.cw = room + 2
		d.markdown(strings.TrimSpace(reply(k)), 1, cSub, false)
		d.cw = cw
		var rows []string
		for _, l := range d.lines[n:] {
			rows = append(rows, strings.TrimRight(strings.TrimPrefix(l.Text, d.spine()), " "))
		}
		d.lines = d.lines[:n]
		if !d.o.Verbose && !d.o.Open[ref] && len(rows) > replyRows {
			rows = append(rows[:replyRows-1], faint("⋯ "+plural(len(rows)-replyRows+1, "more line")))
		}
		d.box(ref, indent, head, "", rows, "", room, cFaint)
	}
}
