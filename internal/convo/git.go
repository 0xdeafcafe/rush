package convo

import (
	"path/filepath"
	"strings"
)

// gitOut is what git prints for cmd when rush colours it: "log" for a
// history, "status" for what's changed; "" for anything else.
func gitOut(cmd string) string {
	f := strings.Fields(strings.Split(cmd, "|")[0])
	if len(f) < 2 || filepath.Base(f[0]) != "git" {
		return ""
	}
	// git's own flags come before the subcommand; -C and -c take a value.
	i := 1
	for ; i < len(f) && strings.HasPrefix(f[i], "-"); i++ {
		if f[i] == "-C" || f[i] == "-c" {
			i++
		}
	}
	if i >= len(f) {
		return ""
	}
	switch f[i] {
	case "log":
		return "log"
	case "status":
		return "status"
	}
	return ""
}

var (
	// A commit a line of a log starts with, after any graph: its hash, the
	// branches and tags on it, and its title.
	gitOneline = lazyRe(`^([*|\\/ ]*)([0-9a-f]{7,40})( \([^)]*\))?( .*)?$`)
	gitHeader  = lazyRe(`^([*|\\/ ]*)(commit )([0-9a-f]{7,40})(.*)$`)
	gitField   = lazyRe(`^([*|\\/ ]*)((?:Author|Date|Merge|Commit|AuthorDate|CommitDate):)(.*)$`)
	// A conventional commit's type, scope and ! before its colon.
	convTitle = lazyRe(`^(\w+)(\([^)]*\))?(!)?(:)( .*)$`)
	// git status --short: two columns of what's changed, then the path.
	gitShort = lazyRe(`^([ MTADRCU?!])([ MTADRCU?!]) (.+)$`)
	// git status: a changed file under its heading.
	gitLong = lazyRe(`^\t(modified|new file|deleted|renamed|copied|typechange|both modified|both added|both deleted|added by us|added by them|deleted by us|deleted by them):(\s+)(.*)$`)
)

// gitStarts is whether l looks like the first line git prints for kind.
func gitStarts(kind, l string) bool {
	switch kind {
	case "log":
		return gitOneline().MatchString(l) || gitHeader().MatchString(l)
	case "status":
		return gitShort().MatchString(l) || strings.HasPrefix(l, "On branch ") || strings.HasPrefix(l, "HEAD detached ") || strings.HasPrefix(l, "## ")
	}
	return false
}

// gitLine colours a line of what git printed for kind: a commit's hash
// amber and its title bold, a conventional commit's type and scope picked
// out, and each changed file's state in the colour of what happened to it.
func (d *drawer) gitLine(kind, l string) string {
	if kind == "status" {
		return gitStatusLine(l)
	}
	if m := gitHeader().FindStringSubmatch(l); m != nil {
		d.subject = true
		return faint(m[1]+m[2]) + paint(cYellow, m[3]) + gitRefs(m[4])
	}
	if m := gitField().FindStringSubmatch(l); m != nil {
		return faint(m[1]+m[2]) + paint(cOut, m[3])
	}
	// A full log's title is the first line of the message under a commit.
	if body, ok := strings.CutPrefix(l, "    "); ok {
		if d.subject && strings.TrimSpace(body) != "" {
			d.subject = false
			return "    " + gitTitle(body)
		}
		return paint(cOut, l)
	}
	if m := gitOneline().FindStringSubmatch(l); m != nil {
		t := ""
		if m[4] != "" {
			t = " " + gitTitle(m[4][1:])
		}
		return faint(m[1]) + paint(cYellow, m[2]) + gitRefs(m[3]) + t
	}
	return paint(cOut, l)
}

// gitRefs is the branches and tags git names after a commit, in brackets.
func gitRefs(s string) string {
	t := strings.TrimSpace(s)
	if len(t) < 2 || t[0] != '(' || t[len(t)-1] != ')' {
		return paint(cOut, s)
	}
	return s[:len(s)-len(strings.TrimLeft(s, " "))] + faint("(") + paint(cGreen, t[1:len(t)-1]) + faint(")")
}

// gitTitle is a commit's title in bold, a conventional commit's type in
// blue, its scope dimmer, and a breaking change's ! red.
func gitTitle(s string) string {
	m := convTitle().FindStringSubmatch(s)
	if m == nil {
		return paint(cSub+bold, s)
	}
	return paint(cBlue, m[1]) + paint(cDim, m[2]) + paint(cRed, m[3]) + faint(m[4]) + paint(cSub+bold, m[5])
}

// gitStatusLine colours a line of git status, short or long.
func gitStatusLine(l string) string {
	if m := gitShort().FindStringSubmatch(l); m != nil {
		return gitState(m[1]) + gitState(m[2]) + " " + gitPath(m[3])
	}
	if m := gitLong().FindStringSubmatch(l); m != nil {
		return "\t" + paint(gitStateColor(m[1][:1]), m[1]+":") + m[2] + gitPath(m[3])
	}
	switch {
	case strings.HasPrefix(l, "## "):
		return faint("## ") + paint(cGreen, l[3:])
	case strings.HasPrefix(l, "\t"): // an untracked file
		return paint(cDim, l)
	case strings.HasPrefix(l, "  ("): // how to undo or commit it
		return faint(l)
	}
	return paint(cOut, l)
}

// gitPath is a changed file, and where a rename took it.
func gitPath(p string) string {
	if a, b, ok := strings.Cut(p, " -> "); ok {
		return paint(cOut, a) + faint(" → ") + paint(cOut, b)
	}
	return paint(cOut, p)
}

func gitState(c string) string {
	if c == " " {
		return c
	}
	return paint(gitStateColor(c), c)
}

// gitStateColor is the colour of a file's state, by its letter in git
// status --short (or the first of its word in the long form).
func gitStateColor(c string) string {
	switch c {
	case "A", "n": // added, new file
		return cGreen
	case "D", "d": // deleted
		return cRed
	case "R", "C", "r", "c": // renamed, copied
		return cBlue
	case "U", "b", "a": // unmerged
		return cOrange
	case "?", "!":
		return cDim
	}
	return cYellow // modified
}
