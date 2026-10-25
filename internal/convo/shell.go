package convo

import (
	"fmt"
	"path/filepath"
	"strings"
)

// shape is what a shell command amounts to when it is really a read, a
// search, a write or a script: the glyph and words its row shows in place
// of the command, and a result the output can't give.
type shape struct {
	kind  string // "read", "search", "write", "script", "commit"; "" for none
	glyph string
	what  string
	res   string
	in    string // the folder a leading cd moves to, when it isn't the session's
	ctx   bool   // a search that prints lines around each match
	n     int    // the most lines a read prints, when that's known
	body  string // what a heredoc writes
}

var sedLines = lazyRe(`^(\d+)(?:,(\d+))?p$`)

// shellShape reads what cmd does. Chains (&&, ;, ||) have no shape, nor
// does a heredoc with more commands after it; a read or a search may pipe
// through filters such as head or grep -v.
func (d *drawer) shellShape(cmd string) shape {
	cmd = joinLines(strings.TrimSpace(cmd))
	var sh shape
	if m := cdRe().FindStringSubmatch(cmd); m != nil {
		dir := strings.Trim(m[1], `"'`)
		cmd = cmd[len(m[0]):]
		if dir != d.s.Info.Cwd && dir != "." {
			sh.in = d.rel(dir)
		}
	}
	lines := strings.Split(cmd, "\n")
	body := 0 // a heredoc's lines, without the one that ends it
	if len(lines) > 1 {
		body = len(lines) - 2
	}
	var toks []string
	for _, t := range shellTokens(lines[0]) {
		if strings.TrimSpace(t) != "" {
			toks = append(toks, t)
		}
	}
	// The first command of a pipeline, and whether anything chains after.
	var args []string
	chained, piped := false, false
	for _, t := range toks {
		switch t {
		case "&&", "||", ";", "&":
			chained = true
		case "|":
			piped = true
		}
		if chained || piped {
			break
		}
		args = append(args, t)
	}
	if len(args) == 0 {
		return sh
	}
	heredoc, end := false, ""
	var into string // the file a > sends output to
	var plain []string
	for i := 1; i < len(args); i++ {
		a := args[i]
		switch {
		case strings.HasPrefix(a, "<<"):
			heredoc = true
			end = strings.TrimLeft(a, "<-")
			if end == "" && i+1 < len(args) {
				i++ // the delimiter word
				end = args[i]
			}
			end = unquote(end)
		case a == ">" || a == ">>":
			if i+1 < len(args) {
				into = unquote(args[i+1])
				i++
			}
		case strings.HasPrefix(a, ">"):
			into = unquote(strings.TrimLeft(a, ">"))
		case a == "2>&1" || strings.HasPrefix(a, "2>"):
		default:
			plain = append(plain, a)
		}
	}
	prog := filepath.Base(args[0])
	if chained || heredoc && strings.TrimSpace(lines[len(lines)-1]) != end {
		return sh
	}
	files := func(xs []string) string {
		var out []string
		for _, x := range xs {
			out = append(out, d.rel(unquote(x)))
		}
		return strings.Join(out, ", ")
	}
	switch prog {
	case "cat", "tee":
		target := into
		if prog == "tee" && len(nonFlags(plain)) > 0 {
			target = unquote(nonFlags(plain)[0])
		}
		switch {
		case heredoc && target != "":
			sh.kind, sh.glyph, sh.what = "write", "✎", d.rel(target)
			sh.res = plural(body, "line")
			sh.body = strings.Join(lines[1:len(lines)-1], "\n")
		case prog == "cat" && into == "" && len(nonFlags(plain)) > 0:
			sh.kind, sh.glyph, sh.what = "read", "◧", files(nonFlags(plain))
		}
	case "nl", "bat", "batcat":
		// nl -ba file, bat file: a read of the file, numbered or not.
		if f := nonFlags(plain); len(f) > 0 && into == "" {
			sh.kind, sh.glyph, sh.what = "read", "◧", d.rel(unquote(f[len(f)-1]))
		}
	case "sed":
		// sed -n 'A,Bp' file: a read of lines A to B.
		if len(plain) >= 3 && plain[0] == "-n" && into == "" {
			sh.kind, sh.glyph, sh.what = "read", "◧", files(plain[2:])
			if m := sedLines().FindStringSubmatch(unquote(plain[1])); m != nil {
				if m[2] == "" {
					sh.res, sh.n = "line "+m[1], 1
				} else {
					sh.res = "lines " + m[1] + "–" + m[2]
					var a, b int
					fmt.Sscan(m[1], &a)
					fmt.Sscan(m[2], &b)
					sh.n = max(0, b-a+1)
				}
			}
		}
	case "head", "tail":
		if f := nonFlags(plain); len(f) > 0 && into == "" && !strings.HasPrefix(f[len(f)-1], "-") {
			last := f[len(f)-1]
			if _, err := fmt.Sscan(last, new(int)); err != nil {
				sh.kind, sh.glyph, sh.what = "read", "◧", d.rel(unquote(last))
				for i, a := range plain {
					switch {
					case a == "-n" && i+1 < len(plain):
						fmt.Sscan(plain[i+1], &sh.n)
					case len(a) > 1 && a[0] == '-' && a[1] >= '0' && a[1] <= '9':
						fmt.Sscan(a[1:], &sh.n)
					}
				}
			}
		}
	case "grep", "egrep", "rg":
		if pat, paths := grepArgs(plain); pat != "" && into == "" {
			sh.kind, sh.glyph, sh.what = "search", "⌕", pat
			for _, a := range plain {
				if len(a) >= 2 && a[0] == '-' && strings.ContainsAny(a[1:2], "ABC") {
					sh.ctx = true
				}
			}
			if len(paths) > 0 {
				sh.what += " in " + files(paths)
			}
		}
	case "python", "python3", "node", "bun", "deno", "ruby", "perl", "bash", "sh", "zsh":
		script := heredoc
		for _, a := range plain {
			if a == "-" || a == "-c" || a == "-e" {
				script = true
			}
		}
		if script {
			sh.kind, sh.glyph, sh.what = "script", "$", prog+" "+scriptGist(lines[1:max(1, len(lines)-1)], heredoc)
			if heredoc {
				sh.res = plural(body, "line")
			}
		}
	case "git":
		if len(plain) > 0 && plain[0] == "commit" {
			sh.kind, sh.glyph, sh.what = "commit", "$", "git commit"
			for i, a := range plain {
				if (a == "-m" || a == "-am") && i+1 < len(plain) {
					sh.res = "“" + oneLine(unquote(plain[i+1])) + "”"
					break
				}
			}
		}
	}
	return sh
}

// span is a stretch of a chain's output: n lines of it (0 for the rest)
// in language lg, or a search's, whose lines each say their own file, a
// diff's, whose added and removed lines are coloured, or git's log or
// status.
type span struct {
	lg     *lang
	n      int
	byPath bool
	diff   bool
	git    string
}

// starts is whether l looks like the first line of sp's output, so that a
// part of unknown length before it ends there.
func (sp span) starts(l string) bool {
	switch {
	case sp.diff:
		return strings.HasPrefix(l, "diff ") || strings.HasPrefix(l, "--- ") || strings.HasPrefix(l, "@@ ")
	case sp.byPath:
		n, _ := codePrefix(l)
		return n > 0
	case sp.git != "":
		return gitStarts(sp.git, l)
	}
	return false
}

// dropHeredocs is cmd without its heredocs' bodies and the << that starts
// each, so the commands around them read as a chain.
func dropHeredocs(cmd string) string {
	if !strings.Contains(cmd, "<<") {
		return cmd
	}
	var out []string
	end := ""
	for _, l := range strings.Split(cmd, "\n") {
		if end != "" {
			if strings.TrimSpace(l) == end {
				end = ""
			}
			continue
		}
		toks := shellTokens(l)
		for i := 0; i < len(toks); i++ {
			if !strings.HasPrefix(toks[i], "<<") {
				continue
			}
			end = strings.TrimLeft(toks[i], "<-")
			toks[i] = ""
			if end == "" {
				for k := i + 1; k < len(toks); k++ {
					if strings.TrimSpace(toks[k]) != "" {
						end, toks[k] = toks[k], ""
						break
					}
				}
			}
			end = unquote(end)
			break
		}
		out = append(out, strings.Join(toks, ""))
	}
	return strings.Join(out, "\n")
}

// joinLines is cmd with the line breaks between its commands as the ; they
// amount to, and continued lines joined. A heredoc keeps its lines, as does
// a quoted string.
func joinLines(cmd string) string {
	if !strings.Contains(cmd, "\n") || strings.Contains(cmd, "<<") {
		return cmd
	}
	var b strings.Builder
	var q byte
	for i := 0; i < len(cmd); i++ {
		c := cmd[i]
		switch {
		case q != 0:
			if c == q {
				q = 0
			}
		case c == '"' || c == '\'':
			q = c
		case c == '\\' && i+1 < len(cmd):
			i++
			if cmd[i] == '\n' {
				b.WriteByte(' ')
			} else {
				b.WriteByte(c)
				b.WriteByte(cmd[i])
			}
			continue
		case c == '\n':
			b.WriteString(" ; ")
			continue
		}
		b.WriteByte(c)
	}
	return b.String()
}

// silent commands print nothing, so a chain's output has no part of theirs.
var silent = map[string]bool{"cd": true, "export": true, "set": true, "unset": true, "mkdir": true, "touch": true, "true": true, ":": true}

// chainSpans is the language of each part of a chain that reads files one
// after another (sed -n 1,200p a.go; cat b.json; git log): a read of known
// length covers that many lines, an echo one, a silent command none, and a
// part of unknown length the rest. Nil when no part is a read.
func (d *drawer) chainSpans(cmd string) []span {
	cmd = joinLines(dropHeredocs(strings.TrimSpace(cmd)))
	if strings.Contains(cmd, "\n") {
		return nil
	}
	var spans []span
	var seg []string
	reads := false
	flush := func() {
		s := strings.TrimSpace(strings.Join(seg, ""))
		seg = nil
		if s == "" {
			return
		}
		prog := filepath.Base(strings.Fields(s)[0])
		if silent[prog] {
			return
		}
		sh := d.shellShape(s)
		sp := span{n: sh.n}
		switch {
		case sh.kind == "read":
			sp.lg = langFor(sh.what)
			reads = reads || sp.lg != nil
		case sh.kind == "search":
			sp.byPath, reads = true, true
			if _, p, ok := strings.Cut(sh.what, " in "); ok {
				sp.lg = langFor(strings.Split(p, ", ")[0])
			}
		case prog == "echo":
			sp.n = 1
		case gitOut(s) != "":
			sp.git, reads = gitOut(s), true
			// git log -p shows each commit's diff under it.
			sp.diff = sp.git == "log" && (strings.Contains(s, " -p") || strings.Contains(s, " --patch"))
		case isDiff(s):
			sp.diff, reads = true, true
			f := strings.Fields(strings.Split(s, "|")[0])[1:]
			if prog == "git" {
				f = f[1:] // diff, show
			}
			for _, a := range nonFlags(f) {
				if sp.lg = langFor(unquote(a)); sp.lg != nil {
					break
				}
			}
		}
		spans = append(spans, sp)
	}
	for _, t := range shellTokens(cmd) {
		switch t {
		case "&&", "||", ";":
			flush()
		default:
			seg = append(seg, t)
		}
	}
	flush()
	if !reads || len(spans) < 2 && !spans[0].diff && spans[0].git == "" {
		return nil
	}
	// A search of unknown length runs to the end, over what the parts
	// after it print too: its own lines say their file, and theirs, when
	// they don't, are in the language of the next part that has one.
	for i := len(spans) - 2; i >= 0; i-- {
		if spans[i].byPath && spans[i].n == 0 && spans[i].lg == nil {
			spans[i].lg = spans[i+1].lg
		}
	}
	return spans
}

// isDiff is whether cmd prints a diff: git diff, git show, diff -u.
func isDiff(cmd string) bool {
	f := strings.Fields(cmd)
	switch filepath.Base(f[0]) {
	case "git":
		return len(f) > 1 && (f[1] == "diff" || f[1] == "show")
	case "diff":
		for _, a := range f[1:] {
			if a == "-u" || strings.HasPrefix(a, "-U") || a == "--unified" {
				return true
			}
		}
	}
	return false
}

// grepArgs splits grep's arguments into its pattern and the paths it looks
// in, skipping flags and the values some of them take.
func grepArgs(args []string) (string, []string) {
	takesValue := map[string]bool{"-e": true, "-A": true, "-B": true, "-C": true, "-m": true, "-g": true, "-t": true, "-f": true, "--glob": true, "--type": true}
	pat := ""
	var paths []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "-e" && i+1 < len(args):
			pat = unquote(args[i+1])
			i++
		case takesValue[a]:
			i++
		case strings.HasPrefix(a, "-"):
		case pat == "":
			pat = unquote(a)
		default:
			paths = append(paths, a)
		}
	}
	// grep's \| is the | it means.
	return strings.ReplaceAll(pat, `\|`, "|"), paths
}

func nonFlags(xs []string) []string {
	var out []string
	for _, x := range xs {
		if !strings.HasPrefix(x, "-") {
			out = append(out, x)
		}
	}
	return out
}

func unquote(s string) string {
	if len(s) >= 2 && (s[0] == '"' || s[0] == '\'') && s[len(s)-1] == s[0] {
		return s[1 : len(s)-1]
	}
	return s
}

// quietTint draws a command in an opened step's well: the programs bright
// so its shape shows, their arguments a shade down, flags, operators and
// redirections quieter still, and quoted text and $vars in their own colours
// so they read as one piece.
func quietTint(cmd string) string { return strings.Join(quietTokens(cmd), "") }

// quietTokens is cmd's words and the spaces between, each tinted as
// quietTint has them, so a row can be broken between words.
func quietTokens(cmd string) []string {
	toks := shellTokens(cmd)
	head, prev, cd := true, "", false
	for i, tok := range toks {
		// shellTokens splits 2>&1 at its &; the pieces stay one redirect.
		redir := strings.HasSuffix(prev, ">") && tok == "&" || prev == "&" && strings.Trim(tok, "0123456789") == "" && tok != ""
		prev = tok
		switch {
		case strings.TrimSpace(tok) == "":
		case cd && !shOps[tok]:
			// Where a cd goes is scaffolding: it reads a shade down.
			toks[i], cd = paint(cDim, tok), false
		case head && tok == "cd":
			toks[i], cd, head = paint(cDim, tok), true, false
		case redir:
			toks[i] = paint(cDim, tok)
		case shOps[tok]:
			toks[i] = paint(cOrange, tok)
			head, cd = true, false
		case head && !strings.Contains(tok, "="):
			toks[i] = paint(cWhite, tok)
			head = false
		case strings.HasPrefix(tok, `"`) || strings.HasPrefix(tok, `'`):
			toks[i] = paint(cGreen, tok)
		case strings.HasPrefix(tok, "$"):
			toks[i] = paint(cBlue, tok)
		case strings.HasPrefix(tok, "-"), redirTok().MatchString(tok):
			toks[i] = paint(cDim, tok)
		default:
			toks[i] = sub(tok)
		}
	}
	return toks
}

// redirTok is a redirection: 2>/dev/null, >out.txt, 2>&1, <in.
var redirTok = lazyRe(`^\d*(>>?|<)(&\d+)?`)

var (
	commitRe   = lazyRe(`(?m)^\[[^\]]*?([0-9a-f]{7,40})\]`)
	goFailName = lazyRe(`(?m)^\s*--- FAIL: (\S+)`)
	blockedRe  = lazyRe(`(?s)<tool_use_error>\s*Blocked:?\s*(.*?)(?:</tool_use_error>|$)`)
	toolErrTag = strings.NewReplacer("<tool_use_error>", "", "</tool_use_error>", "")
)

// blocked is whether a step never ran because the harness refused it.
func blocked(st *Step) bool {
	return st.Status == Failed && blockedRe().MatchString(st.Output)
}

// scriptPath is a quoted path in a script: a/b.go, "x.txt".
var scriptPath = lazyRe(`["']([\w./~-]+\.[A-Za-z]{1,5})["']`)

// scriptGist says what a heredoc script is for: the files it edits or
// reads, else its first line of code.
func scriptGist(body []string, heredoc bool) string {
	if !heredoc {
		return "script"
	}
	src := strings.Join(body, "\n")
	var paths []string
	seen := map[string]bool{}
	for _, m := range scriptPath().FindAllStringSubmatch(src, -1) {
		// .py alone is an extension; .oxlintrc.json is a file.
		if !seen[m[1]] && (!strings.HasPrefix(m[1], ".") || strings.Count(m[1], ".") > 1) {
			seen[m[1]] = true
			paths = append(paths, m[1])
		}
	}
	if len(paths) > 0 {
		verb := "reads "
		if strings.Contains(src, ".write(") || strings.Contains(src, "write_text(") || strings.Contains(src, "writeFileSync(") {
			verb = "edits "
		}
		if len(paths) > 2 {
			paths = append(paths[:2], fmt.Sprintf("+%d", len(paths)-2))
		}
		return "· " + verb + strings.Join(paths, ", ")
	}
	for _, l := range body {
		l = strings.TrimSpace(l)
		if l == "" || strings.HasPrefix(l, "#") || strings.HasPrefix(l, "//") || strings.HasPrefix(l, "import ") || strings.HasPrefix(l, "from ") || strings.HasPrefix(l, "use ") || strings.HasPrefix(l, "require") {
			continue
		}
		return "· " + oneLine(l)
	}
	return "script"
}

// shOpens are the words that start a compound command.
var shOpens = map[string]bool{"while": true, "until": true, "for": true, "if": true, "case": true, "select": true, "function": true}

// compound is whether a command's lines hold a loop, an if, a case, a
// { } group or a function: a script, whose statements aren't a chain's.
func compound(lines []shLine) bool {
	for _, l := range lines {
		if l.verbatim || l.depth > 0 {
			continue
		}
		w, _, _ := strings.Cut(strings.TrimPrefix(strings.TrimPrefix(l.text, "&& "), "|| "), " ")
		if shOpens[w] || w == "{" || strings.HasSuffix(w, "()") {
			return true
		}
	}
	return false
}

// scriptLines lays out a compound command's lines as a script is written:
// a statement a line, deeper inside do, then and { and back out at done,
// fi and }, else and elif at their if's depth, and do, then and a group's
// { at the end of the line they belong to.
func scriptLines(lines []shLine) []shLine {
	var out []shLine
	depth := 0
	add := func(t string, dp int) { out = append(out, shLine{text: t, depth: max(0, dp)}) }
	join := func(s string) {
		if n := len(out); n > 0 && !out[n-1].verbatim {
			out[n-1].text += s
			return
		}
		add(strings.TrimLeft(s, "; "), depth)
	}
	var put func(t string)
	put = func(t string) {
		if t = strings.TrimSpace(t); t == "" {
			return
		}
		w, rest, _ := strings.Cut(t, " ")
		switch w {
		case "do", "then":
			join("; " + w)
			depth++
			put(rest)
		case "else":
			add(w, depth-1)
			put(rest)
		case "elif":
			depth--
			add(t, depth)
		case "done", "fi", "esac", "}":
			depth--
			add(t, depth)
		case "{":
			add(w, depth)
			depth++
			put(rest)
		case "&&", "||":
			if g, ok := strings.CutPrefix(strings.TrimSpace(rest), "{"); ok && (g == "" || g[0] == ' ') {
				join(" " + w + " {")
				depth++
				put(g)
				return
			}
			add(t, depth)
		default:
			if w == "function" || strings.HasSuffix(w, "()") {
				if head, body, ok := strings.Cut(t, "{"); ok {
					add(strings.TrimSpace(head)+" {", depth)
					depth++
					put(body)
					return
				}
			}
			add(t, depth)
			if w == "case" {
				depth++
			}
		}
	}
	for _, l := range lines {
		switch {
		case l.verbatim:
			out = append(out, l)
		case l.depth > 0 && len(out) > 0:
			join(" " + l.text) // a pipe stage goes on its command's line
		default:
			put(l.text)
		}
	}
	return out
}

// What Claude Code says of a command it put in the background, and of the
// folder it's left in.
const (
	bgNotice   = "Command running in background with ID: "
	bgWritten  = "Output is being written to: "
	bgNotified = ". You will be notified"
	cwdNotice  = "Session cwd remains "
)

// notices draws Claude Code's own lines in s (a command put in the
// background, the folder it's left in) as short notes, and gives back the
// rest for output to draw.
func (d *drawer) notices(s string, indent int) string {
	if !strings.Contains(s, bgNotice) && !strings.Contains(s, cwdNotice) {
		return s
	}
	pad := d.spine() + strings.Repeat(" ", indent-1)
	var rest []string
	for _, l := range strings.Split(s, "\n") {
		t := strings.TrimSpace(l)
		switch {
		case strings.HasPrefix(t, bgNotice):
			id, after, _ := strings.Cut(t[len(bgNotice):], ".")
			d.add("", "", pad+dim("↳ running in the background · "+id), "")
			if _, p, ok := strings.Cut(after, bgWritten); ok {
				if k := strings.Index(p, bgNotified); k >= 0 {
					p = p[:k]
				}
				p = strings.TrimSuffix(strings.TrimSpace(p), ".")
				d.addRows("", pad, "  ", d.fileLink(p, faint(p)), d.cw-indent-3, 2)
			}
		case strings.HasPrefix(t, cwdNotice):
			dir := strings.TrimSpace(t[len(cwdNotice):])
			if strings.TrimSuffix(dir, "/") != strings.TrimSuffix(firstNonEmpty(d.s.Info.Cwd, d.s.Cwd), "/") {
				d.add("", "", pad+faint("cwd "+d.rel(dir)), "")
			}
		default:
			rest = append(rest, l)
		}
	}
	return strings.Join(rest, "\n")
}
