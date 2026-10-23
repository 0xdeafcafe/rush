package watch

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// A Live is a running watch as it last stood, kept by the background task
// it runs as, so rush can show what it's waiting on and leave it out of
// long-task check-ins: it ends itself, at its timeout at the latest.
type Live struct {
	Task    string        `json:"task"`
	PID     int           `json:"pid"`
	Repo    string        `json:"repo"`
	Number  int           `json:"number"`
	Title   string        `json:"title,omitempty"`
	Until   Until         `json:"until"`
	Timeout time.Duration `json:"timeout"`
	Start   time.Time     `json:"start"`
	// Asked is when GitHub last answered, zero before it has; Next when
	// the watch asks again.
	Asked   time.Time `json:"asked,omitzero"`
	Next    time.Time `json:"next,omitzero"`
	Limited bool      `json:"limited,omitzero"`
	Passed  int       `json:"passed"`
	Failed  int       `json:"failed"`
	Pending int       `json:"pending"`
	Skipped int       `json:"skipped"`
	// FailedChecks and Running name the checks, failed and still running.
	FailedChecks []string `json:"failedChecks,omitempty"`
	Running      []string `json:"running,omitempty"`
}

// Reports is whether a failure already seen ends the watch: until fail
// and first say one at once, done and each only once all have finished.
func (l Live) Reports() bool { return l.Until == UntilFail || l.Until == UntilFirst }

func liveDir() string { return filepath.Join(Root(), "live") }

// taskOutput is a background task's output file, as Claude Code names it:
// <tasks folder>/<task id>.output.
var taskOutput = regexp.MustCompile(`/tasks/([A-Za-z0-9_-]+)\.output$`)

// TaskID is the background task this process runs as, from where its
// stdout goes; "" when it isn't one.
func TaskID() string {
	m := taskOutput.FindStringSubmatch(stdoutPath())
	if m == nil {
		return ""
	}
	return m[1]
}

// saveLive keeps l for its task, replacing what was there whole.
func saveLive(l Live) {
	if l.Task == "" {
		return
	}
	if err := os.MkdirAll(liveDir(), 0o700); err != nil {
		return
	}
	b, err := jsonx.Marshal(l)
	if err != nil {
		return
	}
	file := filepath.Join(liveDir(), l.Task+".json")
	tmp := file + ".tmp"
	if os.WriteFile(tmp, b, 0o600) == nil {
		_ = os.Rename(tmp, file)
	}
}

func dropLive(task string) {
	if task != "" {
		_ = os.Remove(filepath.Join(liveDir(), task+".json"))
	}
}

// liveGrace is how long past its timeout a watch's record is believed:
// past it, the watch was killed before it could take it away.
const liveGrace = 5 * time.Minute

// LiveFor is the running watch task runs as, if it is one.
func LiveFor(task string, now time.Time) (Live, bool) {
	if task == "" || !validTask(task) {
		return Live{}, false
	}
	b, err := os.ReadFile(filepath.Join(liveDir(), task+".json"))
	if err != nil {
		return Live{}, false
	}
	var l Live
	if jsonx.Unmarshal(b, &l) != nil || l.Task != task {
		return Live{}, false
	}
	if l.Timeout > 0 && now.Sub(l.Start) > l.Timeout+liveGrace {
		return Live{}, false
	}
	if l.PID > 0 && syscall.Kill(l.PID, 0) == syscall.ESRCH {
		return Live{}, false
	}
	return l, true
}

func validTask(task string) bool {
	return !slices.ContainsFunc([]rune(task), func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_' || r == '-')
	})
}

// live is p as the watch with o stands now, about to ask again at next.
func live(o Options, task string, start time.Time, p *PR, asked, next time.Time, limited bool) Live {
	l := Live{
		Task: task, PID: os.Getpid(), Repo: o.Repo, Number: o.Number, Until: o.Until, Timeout: o.Timeout,
		Start: start, Asked: asked, Next: next, Limited: limited,
	}
	if p == nil {
		return l
	}
	l.Title = p.Title
	l.Passed, l.Failed, l.Pending, l.Skipped = p.Counts()
	for _, c := range p.Checks {
		switch c.State {
		case "failed":
			l.FailedChecks = append(l.FailedChecks, c.Name)
		case "pending":
			l.Running = append(l.Running, c.Name)
		}
	}
	return l
}
