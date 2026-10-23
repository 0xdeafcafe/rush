package prwatch

import (
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/watch"
)

// The watch plugin draws its own tasks: a rush watch pr running in the
// background reads as the pull request it waits on, its checks so far,
// and whether it's only waiting or has a failure it will say at the end.
func init() { convo.RegisterJobCard(Card) }

// livesFresh is how long a watch's record, once read, is drawn from: the
// dock draws many times a second, and a watch asks GitHub at most every 15s.
const livesFresh = time.Second

type liveRead struct {
	l  watch.Live
	ok bool
	at time.Time
}

var (
	lives  = map[string]liveRead{}
	livesM sync.Mutex
)

func liveOf(task string, now time.Time) (watch.Live, bool) {
	livesM.Lock()
	defer livesM.Unlock()
	if r, ok := lives[task]; ok && now.Sub(r.at) < livesFresh {
		return r.l, r.ok
	}
	l, ok := watch.LiveFor(task, now)
	for id, r := range lives {
		if now.Sub(r.at) > time.Minute {
			delete(lives, id)
		}
	}
	lives[task] = liveRead{l: l, ok: ok, at: now}
	return l, ok
}

// Card is the watch's card for task j, when j is a rush watch pr.
func Card(j *convo.Job, now time.Time) (convo.JobCard, bool) {
	if j.Kind() != "shell" {
		return convo.JobCard{}, false
	}
	l, ok := liveOf(j.ID, now)
	if !ok {
		return convo.JobCard{}, false
	}
	return card(l, now), true
}

func card(l watch.Live, now time.Time) convo.JobCard {
	c := convo.JobCard{Mark: "◷", Tone: convo.ToneWait, Spin: true, Kind: "watch"}
	seg := func(tone convo.Tone, s string) { c.Body = append(c.Body, convo.Seg{Text: s, Tone: tone}) }
	seg(convo.ToneText, fmt.Sprintf("#%d", l.Number))
	if l.Title != "" {
		seg(convo.ToneSub, " "+clip(l.Title, 36))
	}
	total := l.Passed + l.Failed + l.Pending + l.Skipped
	if total > 0 {
		seg(convo.ToneFaint, "  ")
		c.Body = append(c.Body, bar(l, 12)...)
		seg(convo.ToneGood, fmt.Sprintf(" ✓%d", l.Passed+l.Skipped))
		if l.Failed > 0 {
			seg(convo.ToneBad, fmt.Sprintf(" ✗%d", l.Failed))
		}
		if l.Pending > 0 {
			seg(convo.ToneWait, fmt.Sprintf(" ◌%d", l.Pending))
		}
	}
	seg(convo.ToneFaint, "  ")
	switch {
	case l.Asked.IsZero():
		seg(convo.ToneDim, "asking GitHub…")
	case l.Failed > 0 && !l.Reports() && l.Pending > 0:
		// It has news it won't give until the rest finish.
		c.Mark, c.Tone, c.Spin = "✗", convo.ToneBad, false
		seg(convo.ToneBad, "✗ "+names(l.FailedChecks, 2)+" failed")
		seg(convo.ToneDim, fmt.Sprintf(" · says so once %d more finish", l.Pending))
	case l.Limited:
		seg(convo.ToneWait, "GitHub's rate limit is low: asking less often")
	case total == 0:
		seg(convo.ToneDim, "no checks yet")
	case l.Pending > 0:
		seg(convo.ToneInfo, "waiting on "+names(l.Running, 2))
	default:
		seg(convo.ToneGood, "all finished")
	}
	waited := round(now.Sub(l.Start))
	if l.Timeout > 0 {
		c.Right = append(c.Right, convo.Seg{Text: waited, Tone: convo.ToneSub}, convo.Seg{Text: " of " + round(l.Timeout), Tone: convo.ToneDim})
	} else {
		c.Right = append(c.Right, convo.Seg{Text: waited, Tone: convo.ToneSub})
	}
	if !l.Next.IsZero() {
		if in := l.Next.Sub(now); in > 0 {
			c.Right = append(c.Right, convo.Seg{Text: " · next look " + round(in), Tone: convo.ToneDim})
		} else {
			c.Right = append(c.Right, convo.Seg{Text: " · looking", Tone: convo.ToneInfo})
		}
	}
	c.Detail = detail(l, now)
	return c
}

// bar is the checks as cells, n wide at most: passed, failed, running.
func bar(l watch.Live, n int) []convo.Seg {
	total := l.Passed + l.Failed + l.Pending + l.Skipped
	n = min(n, total)
	cells := func(k int) int {
		if k == 0 {
			return 0
		}
		return max(1, k*n/total)
	}
	bad, wait := cells(l.Failed), cells(l.Pending)
	good := max(0, n-bad-wait)
	var out []convo.Seg
	for _, p := range []struct {
		k    int
		tone convo.Tone
		r    string
	}{{good, convo.ToneGood, "━"}, {bad, convo.ToneBad, "━"}, {wait, convo.ToneFaint, "─"}} {
		if p.k > 0 {
			out = append(out, convo.Seg{Text: strings.Repeat(p.r, p.k), Tone: p.tone})
		}
	}
	return out
}

// detail is what the picked card says under its row.
func detail(l watch.Live, now time.Time) [][]convo.Seg {
	var out [][]convo.Seg
	line := func(segs ...convo.Seg) { out = append(out, segs) }
	if len(l.FailedChecks) > 0 {
		line(convo.Seg{Text: "failed   ", Tone: convo.ToneDim}, convo.Seg{Text: names(l.FailedChecks, 6), Tone: convo.ToneBad})
	}
	if len(l.Running) > 0 {
		line(convo.Seg{Text: "running  ", Tone: convo.ToneDim}, convo.Seg{Text: names(l.Running, 6), Tone: convo.ToneWait})
	}
	ends := map[watch.Until]string{
		watch.UntilDone:  "once every check has finished",
		watch.UntilFail:  "at the first failure, else once all finish",
		watch.UntilFirst: "at the first check to finish",
		watch.UntilEach:  "once all finish, a line as each does",
	}[l.Until]
	when := "ends     "
	if l.Timeout > 0 {
		left := max(0, l.Timeout-now.Sub(l.Start))
		ends += fmt.Sprintf(", or in %s at its timeout", round(left))
	}
	line(convo.Seg{Text: when, Tone: convo.ToneDim}, convo.Seg{Text: "--until " + string(l.Until), Tone: convo.ToneInfo}, convo.Seg{Text: ": " + ends, Tone: convo.ToneSub})
	if !l.Asked.IsZero() {
		line(convo.Seg{Text: "asked    ", Tone: convo.ToneDim}, convo.Seg{Text: fmt.Sprintf("GitHub %s ago · %s#%d", round(now.Sub(l.Asked)), l.Repo, l.Number), Tone: convo.ToneSub})
	}
	return out
}

// names are the first n, and how many more.
func names(all []string, n int) string {
	if len(all) <= n {
		return strings.Join(all, ", ")
	}
	return strings.Join(all[:n], ", ") + fmt.Sprintf(" +%d", len(all)-n)
}

func clip(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n-1]) + "…"
	}
	return s
}

// round is a wait as the row has room for: 40s, 12m, 1h05m.
func round(d time.Duration) string {
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	return fmt.Sprintf("%dh%02dm", int(d.Hours()), int(d.Minutes())%60)
}
