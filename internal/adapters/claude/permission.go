package claude

import (
	"encoding/json/jsontext"
	"io"
	"os"
	"regexp"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// permissionLog is where the allow plugin's log lines go: stderr, which is
// the host's log.
var permissionLog io.Writer = os.Stderr

// summaryMax is how many runes of a call a permission log line keeps.
const summaryMax = 160

// secretPatterns are the shapes of a secret value in a call, each kept up
// to its first group: a NAME_KEY=value, a header's credentials, a flag's
// value, and any long opaque run of letters and digits (a path's slashes
// break the run, so paths stay readable).
var secretPatterns = []*regexp.Regexp{
	regexp.MustCompile(`(?i)(\b[A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD|PASS|AUTH|CREDENTIALS?)[A-Z0-9_]*=)("[^"]*"|'[^']*'|\S+)`),
	regexp.MustCompile(`(?i)(authorization:\s*(?:bearer|basic|token)?\s*)[^\s"']+`),
	regexp.MustCompile(`(?i)(--?(?:password|passwd|token|secret|api-key|key)[ =])("[^"]*"|'[^']*'|\S+)`),
	regexp.MustCompile(`()[A-Za-z0-9_\-+]{32,}={0,2}`),
}

// permissionSummary is a tool call's input in short for a log line: the
// command, path or URL it acts on, on one line, secret values masked and
// cut to summaryMax runes.
func permissionSummary(input jsontext.Value) string {
	var in map[string]any
	_ = jsonx.Unmarshal(input, &in)
	var s string
	for _, k := range []string{"command", "file_path", "notebook_path", "path", "url", "pattern", "plan"} {
		if v, ok := in[k].(string); ok && v != "" {
			s = v
			break
		}
	}
	if s == "" && len(input) > 0 {
		s = string(input)
	}
	s = strings.Join(strings.Fields(s), " ")
	for _, p := range secretPatterns {
		s = p.ReplaceAllString(s, "${1}***")
	}
	if r := []rune(s); len(r) > summaryMax {
		s = string(r[:summaryMax-1]) + "…"
	}
	return s
}
