package convo

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/photon/cellw"
)

// CodeRows draws lines of the file at path as a document: each line's
// number in the margin, nos[i] for lines[i] (none where nos is short), and
// its words coloured as the file's language has them, w cells at most.
func CodeRows(path string, lines []string, nos []int, w int) []string {
	lg := langFor(path)
	numW := 0
	for _, n := range nos {
		numW = max(numW, len(strconv.Itoa(n)))
	}
	var hs hlState
	out := make([]string, 0, len(lines))
	for i, l := range lines {
		pre := ""
		if i < len(nos) && nos[i] > 0 {
			pre = faint(fmt.Sprintf("%*d  ", numW, nos[i]))
		} else if numW > 0 {
			pre = blanks(numW + 2)
		}
		out = append(out, cellw.Truncate(pre+highlight(lg, &hs, expandTabs(l), cText, nil), w, "…"))
	}
	return out
}

// IsCode is whether the file at path is in a language rush colours: a
// document to number, where a log isn't.
func IsCode(path string) bool { return langFor(path) != nil }

var (
	// A script fed on stdin names its own lines: Python's traceback,
	// Ruby's and Perl's errors, node's.
	stdinAt = regexp.MustCompile(`(?m)(?:File "<stdin>", line |^-:|at - line |\[stdin\]:)(\d+)`)
	// The line a traceback ends on says what went wrong.
	faultSays = regexp.MustCompile(`^(?:\w+\.)*\w*(?:Error|Exception|Interrupt|Exit)\b.*`)
)

// stdinFault is the line of a script fed on stdin that a failed run
// stopped at, by its own count, and what went wrong there; 0 when the
// output names none.
func stdinFault(out string) (int, string) {
	line := 0
	for _, m := range stdinAt.FindAllStringSubmatch(out, -1) {
		line, _ = strconv.Atoi(m[1]) // the innermost frame is the last
	}
	if line == 0 {
		return 0, ""
	}
	says := ""
	for _, l := range strings.Split(out, "\n") {
		if l = strings.TrimSpace(l); faultSays.MatchString(l) {
			says = l
		}
	}
	return line, says
}
