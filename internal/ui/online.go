package ui

import (
	"crypto/sha256"
	"encoding/hex"
	"os"
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/netproof"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A Claude Code session rush isn't hosting stops for good when its turn
// dies on an API error: the network down, a connection dropped or stalled,
// the API overloaded. rush tells each to continue, waiting on the API with
// every other session through netproof: one whose prompt cache is still
// warm once the API can be reached, one whose cache has expired (its next
// try re-reads the whole conversation at full price) only on proof the
// connection holds, and then one first.

// onlineEvery is how often rush looks at whether a waiting session may
// try again.
var onlineEvery = 5 * time.Second

// onlineMost is how many times one session is told to continue before
// rush leaves it to you: an API that keeps failing gives up in the end.
const onlineMost = 8

// cacheLife is how long Claude Code's prompt cache lasts: it writes the
// one-hour cache.
const cacheLife = time.Hour

// onlineBackoff is how long after a halt a session is told to continue,
// once it has been told tries times since it last answered: at once the
// first time, then 15s, doubling each time, so an API still failing isn't
// asked again and again.
func onlineBackoff(tries int) time.Duration {
	if tries == 0 {
		return 0
	}
	return 15 * time.Second << (tries - 1)
}

type onlineWatch struct {
	checking bool
	checked  time.Time
	sent     map[string]time.Time // agent key → the halt it was told to continue from
	tries    map[string]int       // agent key → continues sent since it last answered
	// first is when each agent's first halt in a row was: its cache was
	// last warmed before then, whatever its later tries did.
	first    map[string]time.Time
	recorded map[string]time.Time // agent key → the halt last told to netproof
}

// onlineMsg says which waiting agents may try again.
type onlineMsg struct{ goes map[string]bool }

// onlineLook is what one look needs, taken from the model for the look's
// goroutine.
type onlineLook struct {
	failed   []time.Time
	answered bool
	waiting  map[string]bool // agent key → its cache is still warm
}

// continueWaiting is the agents an API error stopped that are due to be
// told to continue, once they may.
func (m *Model) continueWaiting() []*fleet.Agent {
	w := &m.online
	var out []*fleet.Agent
	for _, a := range m.snap.Agents {
		if !a.Continues(m.snap.At) {
			continue
		}
		if w.sent[a.Key].Equal(a.Spend.Halt.At) || w.tries[a.Key] >= onlineMost {
			continue
		}
		if m.snap.At.Before(a.Spend.Halt.At.Add(onlineBackoff(w.tries[a.Key]))) {
			continue
		}
		if q := m.localQ[a.Key]; q != nil && len(q.items) > 0 {
			continue // its queue goes to it instead
		}
		out = append(out, a)
	}
	return out
}

// look gathers what the next look needs: halts not yet shared, whether a
// session told to continue has answered, and who waits, warm or not.
func (m *Model) look() onlineLook {
	w := &m.online
	for _, mp := range []*map[string]time.Time{&w.sent, &w.first, &w.recorded} {
		if *mp == nil {
			*mp = map[string]time.Time{}
		}
	}
	if w.tries == nil {
		w.tries = map[string]int{}
	}
	l := onlineLook{waiting: map[string]bool{}}
	for _, a := range m.snap.Agents {
		h := a.Spend.Halt
		if h == nil {
			if w.tries[a.Key] > 0 {
				l.answered = true // it answered: proof for every other
			}
			delete(w.tries, a.Key)
			delete(w.first, a.Key)
			continue
		}
		if !a.Continues(m.snap.At) {
			continue
		}
		if !w.recorded[a.Key].Equal(h.At) {
			w.recorded[a.Key] = h.At
			l.failed = append(l.failed, h.At)
		}
		if _, ok := w.first[a.Key]; !ok {
			w.first[a.Key] = h.At
		}
	}
	for _, a := range m.continueWaiting() {
		l.waiting[a.Key] = m.snap.At.Before(w.first[a.Key].Add(cacheLife - 5*time.Minute))
	}
	return l
}

// watchOnline shares what's new about the API with every rush process,
// and looks at whether each waiting session may try again. Offline
// (--soak) never does.
func (m *Model) watchOnline() tea.Cmd {
	w := &m.online
	if m.offline || w.checking || time.Since(w.checked) < onlineEvery {
		return nil
	}
	l := m.look()
	if len(l.failed) == 0 && !l.answered && len(l.waiting) == 0 {
		return nil
	}
	w.checking = true
	return func() tea.Msg {
		target := netproof.Target()
		if len(l.failed) > 0 || l.answered {
			events.Log("api-watch", map[string]any{"target": target, "newHalts": len(l.failed), "answered": l.answered, "waiting": len(l.waiting)})
		}
		for _, at := range l.failed {
			netproof.Fail(target, at)
		}
		if l.answered {
			netproof.Answer(target, time.Now())
		}
		goes := map[string]bool{}
		for key, warm := range l.waiting {
			if netproof.MayGo(target, key, warm) {
				goes[key] = true
			}
		}
		return onlineMsg{goes: goes}
	}
}

// onOnline tells each session that may try again to continue: a few
// seconds apart, so they don't all send in the same moment.
func (m *Model) onOnline(msg onlineMsg) tea.Cmd {
	w := &m.online
	w.checking, w.checked = false, time.Now()
	var cmds []tea.Cmd
	i := 0
	for _, a := range m.continueWaiting() {
		if !msg.goes[a.Key] {
			continue
		}
		w.sent[a.Key] = a.Spend.Halt.At
		w.tries[a.Key]++
		m.loader.Nudge(a.Key)
		key, kind, acct, id, name, wait := a.Key, agent.Kind(a.Kind), a.Acct, a.ID, a.DisplayName, time.Duration(i)*3*time.Second
		at, tries, detail := a.Spend.Halt.At, w.tries[a.Key], a.Detail
		i++
		cmds = append(cmds, func() tea.Msg {
			if !claimContinue(key, at) {
				return nil // another rush told it
			}
			time.Sleep(wait)
			err := replyOutside(kind, acct, id, "continue")
			events.Log("api-continue", map[string]any{"key": key, "name": name, "try": tries, "of": onlineMost, "haltAt": at, "why": "the API may be reached again after: " + clipLog(detail), "err": errText(err)})
			if err != nil {
				return doneMsg{err: err}
			}
			return doneMsg{text: "the API is back · " + name + " continues"}
		})
	}
	return tea.Batch(cmds...)
}

// claimContinue claims telling one session to continue from one halt for
// this process, so two rushes open at once don't both send it. Claims a
// few days old are cleared as it goes.
func claimContinue(key string, at time.Time) bool {
	dir := filepath.Join(state.Dir(), "online")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return true
	}
	if old, err := os.ReadDir(dir); err == nil {
		for _, e := range old {
			if st, err := e.Info(); err == nil && time.Since(st.ModTime()) > 72*time.Hour {
				_ = os.Remove(filepath.Join(dir, e.Name()))
			}
		}
	}
	sum := sha256.Sum256([]byte(key + "\x00" + at.UTC().Format(time.RFC3339Nano)))
	f, err := os.OpenFile(filepath.Join(dir, hex.EncodeToString(sum[:8])), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if err != nil {
		return !os.IsExist(err)
	}
	_ = f.Close()
	return true
}
