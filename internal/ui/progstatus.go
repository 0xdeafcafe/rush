package ui

import (
	"crypto/sha1"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"hash/maphash"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// Program status (OSC 7501) tells the terminal what rush's agents are
// doing, for a tab, dock or sidebar that shows it: the whole fleet as the
// root record, and an agent that's working, asking or stopped on an error
// as one each beneath it. Terminals that don't know it ignore it.
// https://www.superlogical.com/rex/docs/build/program-status

// statusMax is how many agents get a record of their own: terminals keep
// at least 64, the root among them.
const statusMax = 63

// ProgramStatusClear removes every record rush made, for when it exits:
// an error record would otherwise outlive it.
const ProgramStatusClear = "\x1b]7501;state=clear\x1b\\"

// programStatus sends the records that changed since it last ran, and
// clears those of agents that no longer have one.
func (m *Model) programStatus() tea.Cmd {
	if m.snap == nil {
		return nil
	}
	// Every message asks, a keystroke too: unchanged, nothing is built.
	print := statusPrint(m.snap.Agents)
	if m.progStatus != nil && print == m.progPrint {
		return nil
	}
	m.progPrint = print
	want := statusRecords(m.snap.Agents)
	var b strings.Builder
	ids := make([]string, 0, len(m.progStatus))
	for id := range m.progStatus {
		if _, ok := want[id]; !ok {
			ids = append(ids, id)
		}
	}
	sort.Strings(ids)
	for _, id := range ids {
		b.WriteString("\x1b]7501;state=clear:id=" + id + "\x1b\\")
		delete(m.progStatus, id)
	}
	ids = ids[:0]
	for id, body := range want {
		if m.progStatus[id] != body {
			ids = append(ids, id)
		}
	}
	sort.Strings(ids) // the root ("") first
	for _, id := range ids {
		b.WriteString("\x1b]7501;" + want[id] + "\x1b\\")
	}
	if b.Len() == 0 {
		return nil
	}
	m.progStatus = want
	return tea.Raw(b.String())
}

var statusSeed = maphash.MakeSeed()

// statusState is what agent a's record reports, "" for none.
func statusState(a *fleet.Agent) (state, kind, msg string) {
	switch {
	case a.NeedsYou() || a.Waiting():
		return "blocked", statusKind(a.Needs), a.Needs
	case a.Live() && !a.Checking:
		return "working", "", a.Detail
	case a.Halted() && !a.Seen:
		return "error", "", a.HaltReason()
	}
	return "", "", ""
}

// statusPrint is a hash of everything statusRecords reads.
func statusPrint(agents []*fleet.Agent) uint64 {
	var h maphash.Hash
	h.SetSeed(statusSeed)
	for _, a := range agents {
		state, _, msg := statusState(a)
		if state == "" {
			continue
		}
		h.WriteString(a.Key)
		h.WriteByte(0)
		h.WriteString(state)
		h.WriteByte(0)
		h.WriteString(msg)
		h.WriteByte(0)
		h.WriteString(a.DisplayName)
		h.WriteByte(0)
		if strings.TrimSpace(a.DisplayName) == "" {
			h.WriteString(agentHandle(a))
		}
		h.WriteByte(1)
	}
	return h.Sum64()
}

// statusRecords is each record's report, by id: "" is the root.
func statusRecords(agents []*fleet.Agent) map[string]string {
	recs := map[string]string{}
	var blocked, working, failed int
	for _, a := range agents {
		state, kind, msg := statusState(a)
		switch state {
		case "blocked":
			blocked++
		case "working":
			working++
		case "error":
			failed++
		default:
			continue
		}
		if len(recs) >= statusMax {
			continue // counted at the root all the same
		}
		id := statusID(a.Key)
		r := "state=" + state + ":id=" + id
		if kind != "" {
			r += ":kind=" + kind
		}
		title := oneLine(a.DisplayName)
		if title == "" {
			title = agentHandle(a) // only then: it isn't cheap
		}
		r += ":title=" + statusText(title, 192)
		if msg = oneLine(msg); msg != "" {
			r += ":msg=" + statusText(msg, 2048)
		}
		recs[id] = r
	}
	root := "state=idle"
	switch {
	case blocked > 0:
		root = "state=blocked:kind=question:msg=" + statusText(fmt.Sprintf("%d agent%s waiting on you", blocked, plural(blocked)), 2048)
	case failed > 0:
		root = "state=error:msg=" + statusText(fmt.Sprintf("%d agent%s stopped on an error", failed, plural(failed)), 2048)
	case working > 0:
		root = "state=working:msg=" + statusText(fmt.Sprintf("%d agent%s working", working, plural(working)), 2048)
	}
	recs[""] = root + ":app=rush:title=" + statusText("rush", 192)
	return recs
}

// statusKind is what an agent asking needs: leave to go on, or an answer.
func statusKind(needs string) string {
	n := strings.ToLower(needs)
	if strings.Contains(n, "permission") || strings.Contains(n, "approv") || strings.Contains(n, "allow") {
		return "permission"
	}
	return "question"
}

// statusID is an agent's record id: its key, hashed to the letters ids
// may use.
func statusID(key string) string {
	h := sha1.Sum([]byte(key))
	return hex.EncodeToString(h[:6])
}

// statusText is s in base64, cut to n bytes first on a rune boundary, its
// control characters gone: a terminal refuses a report that carries one.
func statusText(s string, n int) string {
	s = strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f || r >= 0x80 && r < 0xa0 {
			return -1
		}
		return r
	}, s)
	if len(s) > n {
		s = strings.ToValidUTF8(s[:n], "")
	}
	return base64.StdEncoding.EncodeToString([]byte(s))
}
