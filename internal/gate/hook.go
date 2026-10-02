package gate

import (
	"path/filepath"
	"strings"
)

// Claude Code's Bash calls are gated by a PreToolUse hook that rewrites
// the command to run under `rush gate run`: shims miss programs npx and
// pnpm exec find in node_modules/.bin.

// wrapped marks a command already rewritten.
const wrapped = " gate run --name "

// Gated is the first program in command the rules gate, as the command
// word of any simple command in it, or run by npx, bunx, pnpm exec or
// dlx, or yarn; "" when none is, or it's already wrapped.
func Gated(command string, rules map[string]Rule) string {
	if len(rules) == 0 || strings.Contains(command, wrapped) {
		return ""
	}
	for _, words := range commands(command) {
		for _, p := range programs(words) {
			if _, ok := rules[p]; ok {
				return p
			}
		}
	}
	return ""
}

// Wrap is command run under the gate as name, from dir, by rush at exe.
func Wrap(exe, name, dir, command string) string {
	return quote(exe) + wrapped + quote(name) + " --dir " + quote(dir) + " -- bash -c " + quote(command)
}

// Unwrap is the command Wrap was given, or command as it is.
func Unwrap(command string) string {
	i := strings.Index(command, wrapped)
	if i < 0 {
		return command
	}
	j := strings.Index(command[i:], " -- bash -c '")
	if j < 0 {
		return command
	}
	q := command[i+j+len(" -- bash -c "):]
	if len(q) < 2 || !strings.HasSuffix(q, "'") {
		return command
	}
	s := strings.ReplaceAll(q[1:len(q)-1], `'\''`, `'`)
	if quote(s) != q {
		return command
	}
	return s
}

func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

// skipped come before the program they run: shell words, and wrappers
// whose flags and numbers (nice -n 5, timeout 30) are skipped too.
var skipped = map[string]bool{"if": true, "then": true, "else": true, "elif": true, "do": true, "while": true,
	"until": true, "!": true, "time": true, "nice": true, "nohup": true, "command": true, "builtin": true,
	"exec": true, "env": true, "stdbuf": true, "timeout": true, "sudo": true, "noglob": true}

// programs is the program words runs, and the one a runner in it runs.
func programs(words []string) []string {
	for len(words) > 0 && (skipped[words[0]] || isAssign(words[0]) || isArg(words[0])) {
		words = words[1:]
	}
	if len(words) == 0 {
		return nil
	}
	p, rest := filepath.Base(words[0]), words[1:]
	switch p {
	case "pnpm":
		for i, w := range rest {
			if w == "exec" || w == "dlx" {
				return []string{p, firstProgram(rest[i+1:])}
			}
		}
	case "yarn":
		if len(rest) > 0 && (rest[0] == "exec" || rest[0] == "dlx" || rest[0] == "run") {
			rest = rest[1:]
		}
		return []string{p, firstProgram(rest)}
	case "npx", "bunx", "pnpx":
		return []string{p, firstProgram(rest)}
	}
	return []string{p}
}

func firstProgram(words []string) string {
	for _, w := range words {
		if !strings.HasPrefix(w, "-") {
			return filepath.Base(w)
		}
	}
	return ""
}

func isAssign(w string) bool {
	i := strings.IndexByte(w, '=')
	return i > 0 && !strings.ContainsAny(w[:i], "-/.")
}

// isArg is a wrapper's flag or number.
func isArg(w string) bool { return strings.HasPrefix(w, "-") || w != "" && w[0] >= '0' && w[0] <= '9' }

// commands is the words of each simple command in s, roughly as sh splits
// them; nil when a quote isn't closed.
func commands(s string) [][]string {
	var out [][]string
	var cur []string
	var w strings.Builder
	in := false
	word := func() {
		if in {
			cur, in = append(cur, w.String()), false
			w.Reset()
		}
	}
	end := func() {
		if word(); len(cur) > 0 {
			out, cur = append(out, cur), nil
		}
	}
	for i := 0; i < len(s); i++ {
		switch c := s[i]; {
		case c == '\'':
			j := strings.IndexByte(s[i+1:], '\'')
			if j < 0 {
				return nil
			}
			w.WriteString(s[i+1 : i+1+j])
			in, i = true, i+1+j
		case c == '"':
			j := i + 1
			for ; j < len(s) && s[j] != '"'; j++ {
				if s[j] == '\\' {
					j++
				}
			}
			if j >= len(s) {
				return nil
			}
			w.WriteString(s[i+1 : j])
			in, i = true, j
		case c == '\\' && i+1 < len(s):
			if s[i+1] != '\n' {
				w.WriteByte(s[i+1])
				in = true
			}
			i++
		case c == '$' && i+1 < len(s) && s[i+1] == '{':
			j := strings.IndexByte(s[i:], '}')
			if j < 0 {
				return nil
			}
			w.WriteString(s[i : i+j+1])
			in, i = true, i+j
		case c == '$' && i+1 < len(s) && s[i+1] == '(':
			end()
			i++
		case c == '#' && !in:
			j := strings.IndexByte(s[i:], '\n')
			if j < 0 {
				j = len(s) - i
			}
			i += j - 1
		case c == ' ' || c == '\t':
			word()
		case strings.IndexByte(";&|(){}`\n", c) >= 0:
			end()
		default:
			w.WriteByte(c)
			in = true
		}
	}
	end()
	return out
}

// Commands is the words of each simple command in s, roughly as sh splits
// them; nil when a quote isn't closed.
func Commands(s string) [][]string { return commands(s) }
