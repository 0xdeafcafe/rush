// Package searchguard is the bundled plugin that refuses an agent's
// wasteful searches: a recursive grep, a git grep or rg with a giant
// alternation, a loop of whole-tree git greps. The refusal tells the agent
// the cheaper command. rush gate hook asks it, before gating.
package searchguard

import (
	"context"
	"encoding/json/jsontext"
	"io"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Optional: true, Manifest: Manifest, Run: Run}) }

// Name is the plugin's.
const Name = "search-guard"

// Manifest is the search guard's.
var Manifest = plugin.Manifest{
	Name: Name,
	Description: "Refuses agents' wasteful searches (grep -r, a git grep or rg with dozens of alternatives, " +
		"one whole-tree git grep per name in a loop) and tells them the cheaper command.",
	Command:  []string{"rush"},
	MemoryMB: 16,
}

// On says whether you have the search guard on. It reads the disk.
func On() bool { return plugin.BundledOn(Name) }

// maxAlternatives is the most alternatives one git grep or rg may search.
const maxAlternatives = 15

const (
	recursiveGrep = "search-guard: recursive grep walks everything, nested worktrees and node_modules included. " +
		"Use `rg -n <pattern> <path>` (respects .gitignore, skips nested worktrees) or `git grep -n <pattern>`."
	bigAlternation = "search-guard: a pattern with more than 15 alternatives makes git grep and rg build a huge " +
		"automaton (git grep has used 10-12 GB). Write the words to a file, one per line, and run one pass: " +
		"`rg -n -F -w -f words.txt`."
	grepPerName = "search-guard: this runs one whole-tree git grep per name. Write the names to a file, one per " +
		"line, and run one pass: `rg -n -F -w -f words.txt` (add -o to see which name matched)."
)

// Refusal is why command is a wasteful search, with the cheaper command
// to run instead, or "" when it's fine.
func Refusal(command string) string {
	cmds := gate.Commands(strings.ReplaceAll(command, "{}", xargsArg))
	loop := false
	for _, words := range cmds {
		words = skipLead(words)
		if len(words) == 0 {
			continue
		}
		switch filepath.Base(words[0]) {
		case "for", "while", "until":
			loop = true
		case "done":
			loop = false
		case "grep", "egrep", "fgrep":
			if recursive(words[1:]) {
				return recursiveGrep
			}
		case "rg":
			if alternatives(words[1:]) > maxAlternatives {
				return bigAlternation
			}
		case "git":
			if args, ok := gitGrep(words[1:]); ok {
				if alternatives(args) > maxAlternatives {
					return bigAlternation
				}
				if loop && !hasPathspec(args) {
					return grepPerName
				}
			}
		case "xargs":
			if xargsSearch(words[1:]) {
				return grepPerName
			}
		}
	}
	if scriptedGitGrep.MatchString(command) && scriptLoop.MatchString(command) {
		return grepPerName
	}
	return ""
}

// xargsArg stands in for xargs's {}, which gate.Commands splits at.
const xargsArg = "\x1a"

// xargsValueFlags are xargs's flags that take the next word.
var xargsValueFlags = map[string]bool{"-n": true, "-L": true, "-P": true, "-d": true, "-E": true, "-s": true,
	"-a": true, "--max-args": true, "--max-procs": true, "--delimiter": true, "--arg-file": true}

// xargsSearch is whether xargs's args run a git grep, or an rg searching
// for each input (-I's replacement in its pattern), itself or by sh -c.
func xargsSearch(args []string) bool {
	repl := ""
	for i := 0; i < len(args); i++ {
		switch a := args[i]; {
		case a == "-I" && i+1 < len(args):
			repl, i = args[i+1], i+1
		case a == "-i" || a == "--replace":
			repl = xargsArg
		case strings.HasPrefix(a, "-I") || strings.HasPrefix(a, "-i"):
			repl = a[2:]
		case strings.HasPrefix(a, "--replace="):
			repl = strings.TrimPrefix(a, "--replace=")
		case xargsValueFlags[a]:
			i++
		case strings.HasPrefix(a, "-"):
		default:
			return perInput(args[i:], repl, false)
		}
	}
	return false
}

// perInput is whether words, as xargs runs them with replacement repl,
// search once per input. Under sh -c, rg's "$1" is the input too.
func perInput(words []string, repl string, script bool) bool {
	switch filepath.Base(words[0]) {
	case "git":
		_, ok := gitGrep(words[1:])
		return ok
	case "rg":
		p := rgPattern(words[1:])
		return repl != "" && strings.Contains(p, repl) || script && strings.HasPrefix(p, "$")
	case "sh", "bash", "zsh":
		if c := indexOf(words, "-c"); c > 0 && c+1 < len(words) {
			for _, sub := range gate.Commands(words[c+1]) {
				if sub = skipLead(sub); len(sub) > 0 && perInput(sub, repl, true) {
					return true
				}
			}
		}
	}
	return false
}

// rgValueFlags are rg's common flags that take the next word.
var rgValueFlags = map[string]bool{"-g": true, "--glob": true, "-t": true, "--type": true, "-T": true,
	"--type-not": true, "-A": true, "-B": true, "-C": true, "-m": true, "--max-count": true, "-M": true,
	"-j": true, "-f": true, "--file": true}

// rgPattern is rg's pattern: its first -e's, else its first operand.
func rgPattern(args []string) string {
	first := ""
	for i := 0; i < len(args); i++ {
		switch a := args[i]; {
		case (a == "-e" || a == "--regexp") && i+1 < len(args):
			return args[i+1]
		case strings.HasPrefix(a, "--regexp="):
			return strings.TrimPrefix(a, "--regexp=")
		case a == "--":
			if first == "" && i+1 < len(args) {
				first = args[i+1]
			}
			return first
		case rgValueFlags[a]:
			i++
		case strings.HasPrefix(a, "-"):
		case first == "":
			first = a
		}
	}
	return first
}

// lead come before the program they run.
var lead = map[string]bool{"do": true, "then": true, "else": true, "!": true, "time": true, "command": true,
	"env": true, "sudo": true, "nice": true, "nohup": true, "exec": true}

func skipLead(words []string) []string {
	for len(words) > 0 && (lead[words[0]] || strings.Contains(words[0], "=") && !strings.HasPrefix(words[0], "-")) {
		words = words[1:]
	}
	return words
}

// grepArgFlags are grep's short flags that take a value: the rest of
// their bundle, or the next word.
const grepArgFlags = "efmABCdD"

// recursive is whether grep's args ask it to walk directories.
func recursive(args []string) bool {
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--":
			return false
		case a == "--recursive" || a == "--dereference-recursive" || a == "--directories=recurse":
			return true
		case (a == "-d" || a == "--directories") && i+1 < len(args):
			if args[i+1] == "recurse" {
				return true
			}
			i++
		case strings.HasPrefix(a, "--") || !strings.HasPrefix(a, "-") || len(a) < 2:
		default:
			for j := 1; j < len(a); j++ {
				if a[j] == 'r' || a[j] == 'R' {
					return true
				}
				if strings.IndexByte(grepArgFlags, a[j]) >= 0 {
					if j == len(a)-1 {
						i++
					}
					break
				}
			}
		}
	}
	return false
}

// gitGrep is the args of a git grep, past git's own options.
func gitGrep(args []string) ([]string, bool) {
	for i := 0; i < len(args); i++ {
		switch a := args[i]; {
		case a == "grep":
			return args[i+1:], true
		case a == "-C" || a == "-c" || a == "--git-dir" || a == "--work-tree":
			i++
		case !strings.HasPrefix(a, "-"):
			return nil, false
		}
	}
	return nil, false
}

// alternatives is roughly how many patterns args search for: each -e,
// and each branch of a pattern's | or \|.
func alternatives(args []string) int {
	n := 0
	for _, a := range args {
		if a == "-e" || a == "--regexp" {
			n++
		}
		if c := strings.Count(a, "|"); c > 0 {
			n += c + 1
		}
	}
	return n
}

func hasPathspec(args []string) bool { return indexOf(args, "--") >= 0 }

func indexOf(words []string, w string) int {
	for i, x := range words {
		if x == w {
			return i
		}
	}
	return -1
}

// A git grep run from a node or python script, and a loop around it.
var (
	scriptedGitGrep = regexp.MustCompile("(?:exec(?:File)?Sync|spawnSync|execFile|subprocess\\.\\w+|check_output)\\(\\s*" +
		"(?:[\"'`]git[\"'`]\\s*,\\s*\\[?\\s*[\"'`]grep|[\"'`]git grep|\\[\\s*[\"'`]git[\"'`]\\s*,\\s*[\"'`]grep)")
	scriptLoop = regexp.MustCompile(`\.(?:map|forEach|flatMap|filter)\(|\bfor\s*\(|\bfor\s+\w+\s+(?:of|in)\b|\bwhile\b`)
)

// Run is the plugin on its connection to the broker: the guard runs in
// rush gate hook, so it has nothing to do but be on.
func Run(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}
