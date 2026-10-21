package watch

import (
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Prompt is what the watch plugin tells agents: wait on a pull request's
// checks through rush, which asks GitHub for them, rather than poll it.
const Prompt = `Never wait by polling (sleep loops, re-running gh pr checks, scheduled wakeups, a monitor on a polling script): to wait on a pull request's checks, run rush watch pr [<n>|owner/name#<n>|<url>] --until done|fail|first|each [--timeout 25m]. It waits for you at no cost and prints once, briefly, what happened: the counts, and each failed check with the log lines that say why. It exits 0 when all passed or it merged, 1 when a check failed, 2 when it timed out (run it again), 3 when closed or GitHub couldn't be asked. Run it as a background command where you can, so it wakes you when it ends; --until each prints a line as each check finishes, for a monitor. rush watch help says more.`

// Redirect is what an agent's shell command that polls GitHub for a pull
// request's checks becomes, so that rush waits for it instead: gh pr checks
// --watch, which prints every check again every few seconds, is rewritten
// to rush watch pr; a sleep that then asks GitHub is turned back, with why.
// background and timeout are the call's own (a foreground call is killed
// at its timeout, so the watch ends before it); exe is rush. Both are ""
// for any other command.
func Redirect(command string, background bool, timeout time.Duration, exe string) (rewritten, deny string) {
	if rewritten = rewriteChecks(command, background, timeout, exe); rewritten != "" {
		return rewritten, ""
	}
	if polls(command) {
		return "", "rush waits on a pull request's checks for you, so don't poll GitHub: run " +
			"rush watch pr <n> --until done (or fail, first, each) as a background command; " +
			"it prints once what happened and wakes you when it ends. rush watch help says more."
	}
	return "", ""
}

// rewriteChecks is command as rush watch pr, when it is gh pr checks
// --watch alone, perhaps after a cd.
func rewriteChecks(command string, background bool, timeout time.Duration, exe string) string {
	if exe == "" {
		return ""
	}
	cd, rest := "", strings.TrimSpace(command)
	if first, after, ok := strings.Cut(rest, "&&"); ok && strings.HasPrefix(strings.TrimSpace(first), "cd ") {
		cd, rest = strings.TrimSpace(first)+" && ", strings.TrimSpace(after)
	}
	if strings.ContainsAny(rest, "|;&<>`$()") {
		return ""
	}
	w := strings.Fields(rest)
	if len(w) < 4 || w[0] != "gh" || w[1] != "pr" || w[2] != "checks" {
		return ""
	}
	until, pr, watching := UntilDone, "", false
	for i := 3; i < len(w); i++ {
		switch a := w[i]; {
		case a == "--watch":
			watching = true
		case a == "--fail-fast":
			until = UntilFail
		case a == "--required":
		case a == "-i" || a == "--interval":
			i++ // rush picks how often
		case strings.HasPrefix(a, "--interval="):
		case strings.HasPrefix(a, "-"):
			return "" // a flag rush doesn't do: --json, --web…
		case pr == "":
			pr = a
		default:
			return ""
		}
	}
	if !watching {
		return ""
	}
	// Short of the call's own timeout, so it says how far it got.
	limit := 45 * time.Minute
	if !background {
		if timeout <= 0 {
			timeout = 2 * time.Minute
		}
		limit = max(timeout-15*time.Second, 30*time.Second)
	}
	args := []string{"watch", "pr"}
	if pr != "" {
		args = append(args, pr)
	}
	args = append(args, "--until", string(until), "--timeout", limit.Truncate(time.Second).String())
	for i, a := range args {
		args[i] = quote(a)
	}
	return cd + "echo 'rush: watching through rush watch pr, which waits for you and prints once'; " +
		quote(exe) + " " + strings.Join(args, " ")
}

var (
	sleeps    = regexp.MustCompile(`(^|[\s;&|(])sleep\s+(\d+)`)
	loops     = regexp.MustCompile(`(^|[\s;&|(])(while|until|for)\s`)
	asksForCI = regexp.MustCompile(`\bgh\s+(pr\s+(checks|view|status)|run\s+(list|view|watch)|api\s+\S*(check-runs|/status|actions/runs))`)
)

// polls is whether command waits a while, or in a loop, and asks GitHub
// how a pull request's checks are going.
func polls(command string) bool {
	if !asksForCI.MatchString(command) {
		return false
	}
	long := false
	for _, m := range sleeps.FindAllStringSubmatch(command, -1) {
		if n, _ := strconv.Atoi(m[2]); n >= 20 {
			long = true
		}
	}
	return long || loops.MatchString(command) && sleeps.MatchString(command)
}

func quote(s string) string {
	if s != "" && !strings.ContainsAny(s, " \t\n'\"\\$`!*?[]{}()<>|&;#~") {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}
