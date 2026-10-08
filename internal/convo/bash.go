package convo

import (
	"net/url"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
)

// phrase is one command of a chain in words: a verb and what it acts on.
// Consecutive phrases with the same verb read as one: read a.go, b.go.
type phrase struct {
	verb string
	objs []string
	kind string // "read", "search" or "" for anything else, for the glyph
}

// segment is one simple command of a chain as shellLines lays it out: its
// words, the filters it pipes through and a heredoc's body.
type segment struct {
	text    string
	filters []string
	body    []string
}

// segments splits cmd at ;, &&, || and newlines, keeping each command's
// pipe stages and heredoc with it.
func segments(cmd string) []segment {
	var out []segment
	for _, l := range shellLines(cmd) {
		n := len(out)
		switch {
		case l.verbatim && n > 0:
			out[n-1].body = append(out[n-1].body, l.text)
		case l.depth > 0 && n > 0:
			out[n-1].filters = append(out[n-1].filters, strings.TrimSpace(strings.TrimPrefix(l.text, "|")))
		default:
			t := strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(l.text, "&& "), "|| "))
			out = append(out, segment{text: t})
		}
	}
	return out
}

// full is the segment as a command of its own, for shellShape.
func (s segment) full() string {
	c := s.text
	for _, f := range s.filters {
		c += " | " + f
	}
	if len(s.body) > 0 {
		c += "\n" + strings.Join(s.body, "\n")
	}
	return c
}

// chain is a chain of commands, or one too long to read at a glance, as
// what each of its commands does in colour c; "" for a short single command,
// which reads best as itself.
func (d *drawer) chain(cmd, c string) string {
	k := c + "\x00" + cmd
	if l, ok := d.s.chains[k]; ok {
		return l
	}
	l, ok := d.s.chainsOld[k]
	if !ok {
		cmd := strings.TrimSpace(cmd)
		if strings.ContainsAny(cmd, ";&|\n") || cellw.String(cmd) > 60 {
			if in, ps := d.phrases(cmd); len(ps) > 0 && (len(segments(cmd)) > 1 || cellw.String(cmd) > 60) {
				l = glyphColor(chainGlyph(ps)) + " "
				if in != "" {
					l += faint("in " + tailCells(in, 32) + " · ")
				}
				l += chainLabel(ps, c)
			}
		}
	}
	if d.s.chains != nil {
		d.s.chains[k] = l
	}
	return l
}

// phrases reads a chain of commands as what each one does, leaving out the
// scaffolding (echo separators, exports, loop keywords) and, when the chain
// opens with a cd somewhere else, the folder it works in.
func (d *drawer) phrases(cmd string) (in string, out []phrase) {
	// A line continued with \ is one line.
	segs := segments(strings.ReplaceAll(strings.TrimSpace(cmd), "\\\n", " "))
	loopVar, loopOver := "", ""
	for i, s := range segs {
		words := fieldsOf(s.text)
		if len(words) == 0 {
			continue
		}
		// for f in a b c: the body's $f is a, b, c.
		if words[0] == "for" && len(words) >= 4 && words[2] == "in" {
			loopVar, loopOver = words[1], strings.Join(first(d.paths(words[3:]), 3), ", ")
			continue
		}
		if words[0] == "done" {
			loopVar = ""
		}
		// A loop's body is what it does; for, while and done aren't.
		switch words[0] {
		case "do", "then", "else":
			words = words[1:]
			s.text = strings.TrimSpace(strings.TrimPrefix(strings.TrimPrefix(strings.TrimPrefix(s.text, "do"), "then"), "else"))
		}
		for len(words) > 0 && assignRe.MatchString(words[0]) {
			s.text = strings.TrimSpace(strings.TrimPrefix(s.text, words[0]))
			words = words[1:]
		}
		// What's left of a $(…) or a quoted string split across lines.
		if len(words) == 0 || strings.ContainsAny(words[0][:1], `"'()-|`) || words[0][0] == '$' && !strings.Contains(words[0], "/") {
			continue
		}
		if words[0] == "cd" && len(words) > 1 {
			if i == 0 {
				if dir := unquote(words[1]); dir != d.s.Info.Cwd && dir != "." {
					in = d.rel(dir)
				}
			}
			continue
		}
		p, ok := d.phraseOf(s, words)
		if !ok {
			continue
		}
		if loopVar != "" {
			for k, o := range p.objs {
				if o == "$"+loopVar || o == "${"+loopVar+"}" || o == `"$`+loopVar+`"` {
					p.objs[k] = "each of " + loopOver
				}
			}
		}
		if n := len(out); n > 0 && out[n-1].verb == p.verb && p.verb != "" {
			prev := &out[n-1]
			for _, o := range p.objs {
				// Another range of the file just read: view.go:1–20, 40–60.
				if f, r, ok := strings.Cut(o, ":"); ok && len(prev.objs) > 0 {
					if pf, _, _ := strings.Cut(prev.objs[len(prev.objs)-1], ":"); pf == f && p.kind == "read" {
						prev.objs[len(prev.objs)-1] += ", " + r
						continue
					}
				}
				if !contains(prev.objs, o) {
					prev.objs = append(prev.objs, o)
				}
			}
			continue
		}
		out = append(out, p)
	}
	return in, out
}

var (
	// heredocMsg is the first line of a heredoc inside a command: a commit
	// message given as "$(cat <<'EOF' … EOF)".
	heredocMsg = regexp.MustCompile(`<<-?\s*['"]?\w+['"]?\s*\n\s*([^\n]*\S)`)
	assignRe   = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*=`)
	// redirRe is a redirect between streams, 2>&1 or >&2, which shellTokens
	// would split at its &.
	redirRe = regexp.MustCompile(`\s\d?>&\d\b|\s&>>?\s*\S+`)
)

// fieldsOf is s's words, quotes kept whole, without operators or redirects.
func fieldsOf(s string) []string {
	var out []string
	skip := false
	for _, t := range shellTokens(redirRe.ReplaceAllString(s, "")) {
		switch {
		case strings.TrimSpace(t) == "":
		case skip:
			skip = false
		case t == ">" || t == ">>" || t == "<" || t == "2>":
			skip = true
		case strings.HasPrefix(t, ">") || strings.HasPrefix(t, "2>") || strings.HasPrefix(t, "&>") || strings.HasPrefix(t, "<<"):
		default:
			out = append(out, t)
		}
	}
	return out
}

// quiet commands are a chain's scaffolding, which says nothing of what it did.
var quiet = map[string]bool{
	"echo": true, "printf": true, "export": true, "set": true, "true": true, "false": true, ":": true,
	"for": true, "while": true, "until": true, "done": true, "if": true, "fi": true, "elif": true, "esac": true,
	"source": true, ".": true, "unset": true, "local": true, "{": true, "}": true, "wait": true, "exit": true,
	"[": true, "[[": true, "test": true, "continue": true, "break": true, "return": true, "case": true, "read": true, "shift": true,
	"pwd": true, "clear": true, "date": true, "cat": true,
}

// runners run the command that follows them: npx tsc, uv run pytest,
// xargs grep.
var runners = map[string]int{"npx": 1, "bunx": 1, "pnpx": 1, "xargs": 1, "time": 1, "env": 1, "nice": 1, "sudo": 1, "timeout": 2, "uv run": 2, "poetry run": 2, "pnpm exec": 2, "bun run": 2}

func (d *drawer) phraseOf(s segment, words []string) (phrase, bool) {
	// A script given inline (-c) is read below, where its code is at hand.
	if sh := d.shellShape(s.full()); sh.kind != "" && !(sh.kind == "script" && len(s.body) == 0) {
		switch sh.kind {
		case "read":
			obj := sh.what
			if r := strings.TrimPrefix(strings.TrimPrefix(sh.res, "lines "), "line "); r != "" && !strings.Contains(obj, ", ") {
				obj += ":" + r
			}
			return phrase{verb: "read", objs: []string{obj}, kind: "read"}, true
		case "search":
			return phrase{verb: "search", objs: []string{sh.what}, kind: "search"}, true
		case "write":
			return phrase{verb: "write", objs: []string{sh.what}}, true
		case "script":
			prog := filepath.Base(words[0])
			return phrase{verb: prog, objs: []string{strings.TrimPrefix(strings.TrimPrefix(sh.what, prog+" "), "· ")}}, true
		case "commit":
			res := sh.res
			// A message fed in by heredoc: its first line.
			if strings.Contains(res, "$(") {
				res = ""
				if m := heredocMsg.FindStringSubmatch(s.text); m != nil {
					res = "“" + strings.TrimSpace(m[1]) + "”"
				}
				for _, l := range s.body {
					if res != "" {
						break
					}
					if l = strings.TrimSpace(l); l != "" {
						res = "“" + l + "”"
						break
					}
				}
			}
			return phrase{verb: "git commit", objs: nonEmpty(res)}, true
		}
	}
	prog := filepath.Base(words[0])
	if quiet[prog] {
		return phrase{}, false
	}
	args := words[1:]
	// A runner's command is the one that says what happens.
	two := prog
	if len(args) > 0 {
		two += " " + args[0]
	}
	if n, ok := runners[two]; ok || runners[prog] > 0 {
		if !ok {
			n = runners[prog]
		}
		rest := words[n:]
		for len(rest) > 0 && (strings.HasPrefix(rest[0], "-") || assignRe.MatchString(rest[0]) || prog == "timeout" && len(rest) > 1 && rest[0][0] >= '0' && rest[0][0] <= '9') {
			rest = rest[1:]
		}
		if len(rest) > 0 {
			return d.phraseOf(segment{text: strings.Join(rest, " "), filters: s.filters, body: s.body}, rest)
		}
	}
	plain := d.paths(nonFlags(args))
	switch prog {
	case "sleep":
		if len(args) > 0 {
			return phrase{verb: "wait", objs: []string{strings.TrimSuffix(args[0], "s") + "s"}}, true
		}
	case "ls":
		return phrase{verb: "list", objs: plain}, true
	case "find", "fd":
		return phrase{verb: "find", objs: findObjs(d, args)}, true
	case "wc":
		return phrase{verb: "count", objs: plain}, true
	case "rm", "rmdir", "trash":
		return phrase{verb: "delete", objs: plain}, true
	case "mkdir":
		return phrase{verb: "mkdir", objs: plain}, true
	case "cp", "mv", "ln":
		verb := map[string]string{"cp": "copy", "mv": "move", "ln": "link"}[prog]
		if len(plain) >= 2 {
			return phrase{verb: verb, objs: []string{strings.Join(plain[:len(plain)-1], ", ") + " → " + plain[len(plain)-1]}}, true
		}
		return phrase{verb: verb, objs: plain}, true
	case "touch", "diff", "tar", "unzip", "open", "which", "stat", "du", "file":
		return phrase{verb: prog, objs: plain}, true
	case "kill", "pkill", "killall":
		return phrase{verb: "stop", objs: plain}, true
	case "ps", "lsof", "pgrep", "top":
		return phrase{verb: prog}, true
	case "curl", "wget", "http", "xh":
		for _, a := range args {
			if u, err := url.Parse(unquote(a)); err == nil && u.Host != "" {
				return phrase{verb: "fetch", objs: []string{u.Host + strings.TrimSuffix(u.Path, "/")}}, true
			}
		}
		return phrase{verb: "fetch"}, true
	case "git":
		return gitPhrase(d, args), true
	case "sed", "awk", "jq", "perl", "yq", "gawk":
		files := plain
		if len(files) > 0 {
			files = files[1:] // the program
		}
		for _, a := range args {
			if a == "-i" || strings.HasPrefix(a, "-i.") || a == "-pi" {
				return phrase{verb: "edit", objs: first(files, 3)}, true
			}
		}
		return phrase{verb: prog, objs: first(files, 2)}, true
	case "chmod", "chown":
		if len(plain) > 1 {
			plain = plain[1:]
		}
		return phrase{verb: prog, objs: first(plain, 2)}, true
	case "go", "cargo", "make", "just", "docker", "kubectl", "gh", "brew", "uv", "pip", "pip3":
		return subPhrase(d, prog, args), true
	case "npm", "pnpm", "yarn", "bun", "npx", "bunx", "pnpx":
		return subPhrase(d, prog, args), true
	case "python", "python3", "node", "ruby", "bash", "sh", "zsh", "deno":
		for i, a := range args {
			if (a == "-c" || a == "-e" || a == "-p") && i+1 < len(args) {
				code := strings.FieldsFunc(unquote(args[i+1]), func(r rune) bool { return r == '\n' || r == ';' })
				return phrase{verb: prog, objs: []string{strings.TrimPrefix(scriptGist(code, true), "· ")}}, true
			}
		}
		if f := nonFlags(args); len(f) > 0 {
			return phrase{verb: prog, objs: []string{d.rel(unquote(f[0]))}}, true
		}
		return phrase{verb: prog}, true
	}
	return phrase{verb: prog, objs: first(plain, 2)}, true
}

// gitPhrase is a git command as its subcommand and what it names: git diff
// main, git add a.go, git log.
func gitPhrase(d *drawer, args []string) phrase {
	for len(args) >= 2 && (args[0] == "-C" || args[0] == "-c") {
		args = args[2:]
	}
	if len(args) == 0 {
		return phrase{verb: "git"}
	}
	sub := args[0]
	objs := d.paths(nonFlags(args[1:]))
	switch sub {
	case "status", "log", "fetch", "pull", "push", "stash", "branch", "remote", "worktree", "rev-parse", "ls-files":
		// What they name is rarely the point.
		if sub == "log" || sub == "worktree" || sub == "stash" || sub == "branch" {
			objs = first(objs, 1)
		} else {
			objs = nil
		}
	case "grep":
		if pat, paths := grepArgs(args[1:]); pat != "" {
			o := pat
			if len(paths) > 0 {
				o += " in " + strings.Join(d.paths(paths), ", ")
			}
			return phrase{verb: "search", objs: []string{o}, kind: "search"}
		}
	}
	return phrase{verb: "git " + sub, objs: first(objs, 2)}
}

// takesValue are flags of package managers and the like whose next word is
// their value, not a subcommand.
var takesValue = map[string]bool{"--filter": true, "-F": true, "-C": true, "--dir": true, "--prefix": true, "-w": true, "--workspace": true, "-f": true, "--file": true, "-p": true, "--project": true, "-R": true, "--repo": true, "-n": true, "--namespace": true, "--manifest-path": true,
	"-run": true, "-bench": true, "-count": true, "-benchtime": true, "-timeout": true, "-tags": true, "-o": true, "-skip": true, "--json": true, "-q": true, "--jq": true, "--job": true}

// subPhrase is a tool with subcommands: go test ./..., pnpm typecheck,
// gh pr view 12. A workspace filter reads as where it ran.
func subPhrase(d *drawer, prog string, args []string) phrase {
	var words []string
	where := ""
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case takesValue[a] && i+1 < len(args):
			if a == "--filter" || a == "-F" || a == "-w" || a == "--workspace" {
				where = unquote(args[i+1])
			}
			i++
		case strings.HasPrefix(a, "--filter="):
			where = unquote(strings.TrimPrefix(a, "--filter="))
		case strings.HasPrefix(a, "-"):
		default:
			words = append(words, unquote(a))
		}
	}
	verb := prog
	subs := 1
	switch prog {
	case "gh", "kubectl", "docker":
		subs = 2 // gh pr view, kubectl get pods
	case "npm", "yarn", "bun":
		if len(words) > 0 && words[0] == "run" {
			words = words[1:]
		}
	case "pnpm":
		if len(words) > 0 && (words[0] == "run" || words[0] == "exec") {
			words = words[1:]
		}
	case "npx", "bunx", "pnpx":
		subs = 1
	}
	for i := 0; i < subs && len(words) > 0; i++ {
		verb += " " + words[0]
		words = words[1:]
	}
	many := 2
	if prog == "gh" || prog == "kubectl" {
		many = 1 // the rest are mostly flag values
	}
	var kept []string
	for _, w := range words {
		if w != "true" && w != "false" && !strings.ContainsAny(w, "{}\n") {
			kept = append(kept, w)
		}
	}
	objs := first(d.paths(kept), many)
	switch {
	case where != "" && len(objs) > 0:
		objs[len(objs)-1] += " in " + where
	case where != "":
		objs = []string{"in " + where}
	}
	return phrase{verb: verb, objs: objs}
}

// findObjs is what find looks for, and where: *.go in internal.
func findObjs(d *drawer, args []string) []string {
	what, where := "", ""
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case (a == "-name" || a == "-iname" || a == "-path") && i+1 < len(args):
			what = unquote(args[i+1])
			i++
		case strings.HasPrefix(a, "-"):
			// A predicate's value.
			if i+1 < len(args) && !strings.HasPrefix(args[i+1], "-") && a != "-print" && a != "-delete" {
				i++
			}
		case where == "":
			where = d.rel(unquote(a))
		}
	}
	switch {
	case what != "" && where != "" && where != ".":
		return []string{what + " in " + where}
	case what != "":
		return []string{what}
	case where != "":
		return []string{where}
	}
	return nil
}

// paths are args as a row shows them: unquoted, inside the session's folder
// relative to it.
func (d *drawer) paths(args []string) []string {
	out := make([]string, 0, len(args))
	for _, a := range args {
		// A format or a pattern given to a flag isn't a path, nor what's
		// left of an operator.
		if a = unquote(a); strings.Contains(a, "%") || strings.Trim(a, "|&();") == "" || a == "." || a == "./" {
			continue
		}
		out = append(out, d.rel(a))
	}
	return out
}

func first(xs []string, n int) []string {
	if len(xs) > n {
		return append(xs[:n:n], "…")
	}
	return xs
}

func nonEmpty(s string) []string {
	if s == "" {
		return nil
	}
	return []string{s}
}

func contains(xs []string, s string) bool {
	for _, x := range xs {
		if x == s {
			return true
		}
	}
	return false
}

// objWidth is as much of one thing a phrase names as a row gives it.
const objWidth = 48

// chainLabel draws phrases in the row's colour c, the verbs a shade under
// what they act on, one after another.
func chainLabel(ps []phrase, c string) string {
	var parts []string
	for _, p := range ps {
		objs := make([]string, 0, len(p.objs))
		for _, o := range p.objs {
			objs = append(objs, shortObj(o))
		}
		s := faint(p.verb)
		if len(objs) > 0 {
			s += " " + paint(c, strings.Join(objs, ", "))
		}
		parts = append(parts, s)
	}
	return strings.Join(parts, faint(" › "))
}

// shortObj fits one thing a phrase names in objWidth: a long path keeps
// its end, where the file's name is.
func shortObj(o string) string {
	o = oneLine(o)
	if cellw.String(o) <= objWidth {
		return o
	}
	if strings.Contains(o, "/") && !strings.Contains(o, " ") {
		return tailCells(o, objWidth)
	}
	// pattern in some/long/path: the pattern, then the path's end.
	if i := strings.LastIndex(o, " in "); i > 0 {
		head := truncateCells(o[:i], objWidth/2)
		return head + " in " + tailCells(o[i+4:], max(12, objWidth-cellw.String(head)-4))
	}
	return truncateCells(o, objWidth)
}

// tailCells is s's last w cells, led by … when it had more.
func tailCells(s string, w int) string {
	r := []rune(s)
	for len(r) > 0 && cellw.String(string(r)) > w-1 {
		r = r[1:]
	}
	if len(r) == len([]rune(s)) {
		return s
	}
	return "…" + string(r)
}

// chainGlyph is the glyph for a chain: a read or a search when that's all
// it does, else the shell's.
func chainGlyph(ps []phrase) string {
	kind := ""
	for _, p := range ps {
		if p.kind == "" {
			return "$"
		}
		if kind != "" && kind != p.kind {
			return "◧"
		}
		kind = p.kind
	}
	if kind == "search" {
		return "⌕"
	}
	return "◧"
}

// echoMarks are the strings a command echoes on their own, which agents
// print between the parts of a long command's output: === tests ===, ---.
func echoMarks(cmd string) map[string]bool {
	var marks map[string]bool
	for _, s := range segments(cmd) {
		w := fieldsOf(s.text)
		if len(w) < 2 || w[0] != "echo" || len(s.filters) > 0 {
			continue
		}
		m := strings.TrimSpace(unquote(strings.Join(w[1:], " ")))
		if m == "" || strings.ContainsAny(m, "$`") {
			continue
		}
		if marks == nil {
			marks = map[string]bool{}
		}
		marks[m] = true
	}
	return marks
}

var markTrim = "=-#*~_:[]<> \t"

// markHeading is an echoed marker as a heading in the output: its words
// between rules, or a plain rule when it has none.
func markHeading(m string, w int) string {
	words := strings.Trim(m, markTrim)
	if words == "" {
		return faint(strings.Repeat("─", min(w, 24)))
	}
	return faint("── ") + sub(truncateCells(words, max(8, w-8))) + faint(" ──")
}
