package convo

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// ShellWrite is a file a shell command writes, as the command shows it.
// Content is what it writes, when the command holds all of it (a heredoc
// sent to the file).
type ShellWrite struct {
	Path    string
	Content string
	Whole   bool // Content is all the file holds now
}

// heredocOpen is a heredoc's opening: <<EOF, <<-'EOF', << "EOF".
var heredocOpen = regexp.MustCompile(`<<-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?`)

// ShellWrites are the files shell command cmd plainly writes, run in cwd:
// redirects, tee, touch, the target of cp, mv, ln and rsync, files edited
// in place (sed -i, perl -i, gofmt -w, prettier --write), and the files a
// script fed on stdin is given (python3 - a.go b.go), which is how agents
// edit with a script. A cd in it moves where the rest runs.
// shortcut: a word-split read, not a shell parser: quotes with spaces in,
// variables and globs fool it, and a script's file arguments may only be
// read. Enough to show most of what a session changes outside its edits.
func ShellWrites(cmd, cwd string) []ShellWrite {
	w, _, _ := ShellRun(cmd, cwd)
	return w
}

// ShellRun is ShellWrites, with the files the command removes (rm) and
// the folder it ends in: an agent's shell that keeps its folder between
// commands runs the next there.
func ShellRun(cmd, cwd string) (out []ShellWrite, removed []string, end string) {
	seen := map[string]int{}
	keep := func(p, content string, whole bool) {
		if p == "" || p == "-" || strings.ContainsAny(p, "$*?`(){}<>|") || strings.HasPrefix(p, "/dev/") || strings.HasPrefix(p, "&") {
			return
		}
		if p = resolve(p, cwd); p == "" {
			return
		}
		if i, ok := seen[p]; ok {
			if whole {
				out[i].Content, out[i].Whole = content, true
			}
			return
		}
		seen[p] = len(out)
		out = append(out, ShellWrite{Path: p, Content: content, Whole: whole})
	}
	for _, line := range commandLines(cmd) {
		for _, seg := range shellSegs(line.text) {
			f := seg.words
			for len(f) > 0 && (strings.Contains(f[0], "=") || f[0] == "sudo" || f[0] == "env" || f[0] == "time" || f[0] == "command" || f[0] == "rtk") {
				f = f[1:]
			}
			if len(f) == 0 {
				continue
			}
			if f[0] == "cd" {
				if len(f) > 1 && !strings.ContainsAny(f[1], "$`") {
					d := strings.Trim(f[1], `"'`)
					if !filepath.IsAbs(d) && cwd != "" {
						d = filepath.Join(cwd, d)
					}
					if filepath.IsAbs(d) {
						cwd = filepath.Clean(d)
					}
				}
				continue
			}
			var args []string // what isn't a flag or a redirect
			var redirected []string
			for i := 0; i < len(f); i++ {
				w := strings.TrimLeft(f[i], "0123456789&")
				switch {
				case seg.quoted[i]:
					if i > 0 {
						args = append(args, f[i])
					}
				case w == ">" || w == ">>" || w == ">|":
					if i+1 < len(f) {
						redirected = append(redirected, f[i+1])
						i++
					}
				case strings.HasPrefix(w, ">") && !strings.HasPrefix(w, ">&"):
					redirected = append(redirected, strings.TrimLeft(w, ">|"))
				case strings.HasPrefix(f[i], "<"):
					if f[i] == "<" || f[i] == "<<" || f[i] == "<<-" || f[i] == "<<<" {
						i++ // its word is read, not written
					}
				case i == 0:
				case strings.HasPrefix(w, "-") && w != "-":
				default:
					args = append(args, f[i])
				}
			}
			for _, r := range redirected {
				// cat > f <<EOF: the heredoc is the whole file.
				whole := line.body != "" && f[0] == "cat" && !seg.appends
				keep(r, line.body, whole)
			}
			prog := filepath.Base(f[0])
			switch prog {
			case "rm":
				for _, a := range args {
					if p := resolve(a, cwd); p != "" && !strings.ContainsAny(a, "$*?`") {
						removed = append(removed, p)
					}
				}
			case "touch", "tee":
				for _, a := range args {
					keep(a, "", false)
				}
			case "cp", "mv", "ln", "rsync", "install":
				if len(args) >= 2 {
					keep(args[len(args)-1], "", false)
				}
			case "sed", "gsed", "perl":
				if inPlace(f, "-i") || inPlace(f, "-pi") {
					for _, a := range inPlaceFiles(f) {
						keep(a, "", false)
					}
				}
			case "gofmt", "goimports", "gofumpt":
				if inPlace(f, "-w") {
					for _, a := range args {
						keep(a, "", false)
					}
				}
			case "prettier", "black", "rustfmt":
				if prog != "prettier" || inPlace(f, "--write") {
					for _, a := range args {
						keep(a, "", false)
					}
				}
			case "python", "python3", "node", "ruby", "perl5":
				// A script on stdin, edit-in-place style: the files it's given.
				if dash := indexOf(f, "-"); dash > 0 && line.body != "" {
					for _, a := range f[dash+1:] {
						if !strings.HasPrefix(a, "-") && !strings.HasPrefix(a, "<") && looksLikeFile(a) {
							keep(a, "", false)
						}
					}
				}
			}
		}
	}
	return out, removed, cwd
}

// resolve is path p, from cwd, made whole: "" for home (the Left ledger
// has those), temp folders, or a relative path with nowhere to be from.
func resolve(p, cwd string) string {
	if strings.HasPrefix(p, "~/") {
		return ""
	}
	if !filepath.IsAbs(p) {
		if cwd == "" {
			return ""
		}
		p = filepath.Join(cwd, p)
	}
	p = filepath.Clean(p)
	if scratch(p) {
		return ""
	}
	return p
}

// scratch is whether p is in a temp folder: thrown away, not changed.
func scratch(p string) bool {
	for _, d := range []string{"/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/", filepath.Clean(os.TempDir()) + "/"} {
		if strings.HasPrefix(p, d) {
			return true
		}
	}
	return false
}

// inPlace is whether the command's words have flag, alone or with a
// suffix (sed -i.bak, -i”).
func inPlace(f []string, flag string) bool {
	for _, w := range f[1:] {
		if w == flag || strings.HasPrefix(w, flag) && len(flag) == 2 {
			return true
		}
	}
	return false
}

// inPlaceFiles are the files sed or perl edits in place: its words less
// flags, the script (-e's word, or else the first) and macOS's empty
// backup suffix.
func inPlaceFiles(f []string) []string {
	var files []string
	script := false
	for i := 1; i < len(f); i++ {
		w := f[i]
		switch {
		case w == "":
		case w == "-e" || w == "--expression":
			i++
			script = true
		case strings.HasPrefix(w, "-"):
		case !script:
			script = true
		default:
			files = append(files, w)
		}
	}
	return files
}

func indexOf(f []string, w string) int {
	for i, x := range f {
		if x == w {
			return i
		}
	}
	return -1
}

// looksLikeFile is whether a word names a file: a path, or a name with
// an extension.
func looksLikeFile(a string) bool {
	base := filepath.Base(a)
	return strings.Contains(a, "/") || strings.Contains(base, ".") && !strings.HasPrefix(base, ".")
}

// cmdLine is one line of a command, and the heredoc that follows it, if
// it opens one.
type cmdLine struct{ text, body string }

// commandLines are cmd's lines with heredoc bodies taken out: a script's
// lines aren't commands.
func commandLines(cmd string) []cmdLine {
	lines := strings.Split(cmd, "\n")
	var out []cmdLine
	for i := 0; i < len(lines); i++ {
		l := cmdLine{text: lines[i]}
		if m := heredocOpen.FindStringSubmatch(l.text); m != nil {
			var body []string
			for i++; i < len(lines) && strings.TrimSpace(lines[i]) != m[1]; i++ {
				body = append(body, lines[i])
			}
			l.body = strings.Join(body, "\n") + "\n"
		}
		out = append(out, l)
	}
	return out
}

// shellSeg is one simple command of a line: its words, quotes taken off,
// and which of them were quoted (a quoted > is a word, not a redirect).
type shellSeg struct {
	words   []string
	quoted  []bool
	appends bool // it has a >>
}

// shellSegs splits a command line at &&, ||, ; and | outside quotes, and
// each part into its words.
func shellSegs(line string) []shellSeg {
	var out []shellSeg
	var cur shellSeg
	var w strings.Builder
	inWord, quoted := false, false
	end := func() {
		if inWord {
			cur.words, cur.quoted = append(cur.words, w.String()), append(cur.quoted, quoted)
			if w.String() == ">>" && !quoted {
				cur.appends = true
			}
		}
		w.Reset()
		inWord, quoted = false, false
	}
	var q rune
	rs := []rune(line)
	for i := 0; i < len(rs); i++ {
		r := rs[i]
		switch {
		case q != 0:
			if r == q {
				q = 0
			} else if r == '\\' && q == '"' && i+1 < len(rs) {
				i++
				w.WriteRune(rs[i])
			} else {
				w.WriteRune(r)
			}
		case r == '\'' || r == '"':
			q, inWord, quoted = r, true, true
		case r == '\\' && i+1 < len(rs):
			i++
			w.WriteRune(rs[i])
			inWord = true
		case r == ' ' || r == '\t':
			end()
		case r == ';' || r == '|' || r == '&' && i+1 < len(rs) && rs[i+1] == '&':
			if r == '&' || r == '|' && i+1 < len(rs) && rs[i+1] == '|' {
				i++
			}
			end()
			out = append(out, cur)
			cur = shellSeg{}
		case r == '#' && !inWord:
			end()
			return append(out, cur)
		default:
			w.WriteRune(r)
			inWord = true
		}
	}
	end()
	return append(out, cur)
}
