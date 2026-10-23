package host

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/watch"
)

// A background task can be left running long after it's any use: a
// monitor waiting on what never comes, a loop polling a queue that moved
// on. Rush never stops one; it asks the agent to look, since it knows
// what each was for. Subagents and workflows aren't asked about: they're
// working, rush shows their steps, and the agent hears when they end, so a
// check-in only buys a turn that says "it's fine".
const (
	longTaskAfter = 30 * time.Minute // a background task running this long is asked about
	longTaskAgain = time.Hour        // and a session is asked at most this often
	// A task asked about once is asked about again only this long after:
	// most were fine (a server kept up, a long build), and said so.
	longTaskReask = 4 * time.Hour
)

// longTaskLast is the key in nudged for when the session was last asked.
const longTaskLast = ""

// watchLongTasks asks the agent about its long-running background tasks,
// once a minute.
func (s *server) watchLongTasks() {
	t := time.NewTicker(time.Minute)
	defer t.Stop()
	for {
		select {
		case <-s.quit:
			return
		case now := <-t.C:
			s.mu.Lock()
			conn := s.conn
			s.mu.Unlock()
			// An agent that keeps its own task records ends from them those
			// it never said had ended.
			var ended []string
			if e, ok := conn.(interface{ EndTasks() []string }); ok {
				ended = e.EndTasks()
			}
			s.mu.Lock()
			var due []Task
			// Not while it asks you something: the message would answer it.
			// Nor at a usage limit: it can't answer, and each try is a
			// failed turn.
			held := ""
			switch {
			case s.conn == nil || s.info.Sleeping:
			case s.info.State == "blocked" || len(s.pending) > 0:
				held = "it's asking you something"
			case s.info.Limit != nil:
				held = "it's at a usage limit"
			default:
				due = slices.DeleteFunc(overdueTasks(s.askable(), s.nudged, now), func(t Task) bool { return slices.Contains(ended, t.ID) })
			}
			if held != "" && len(overdueTasks(s.askable(), s.nudged, now)) == 0 {
				held = "" // nothing it would be asked about anyway
			}
			if held != s.checkInHeld {
				s.checkInHeld = held
				if held != "" {
					s.event("check-in-held", map[string]any{"why": held})
				}
			}
			if len(due) > 0 && s.nudged == nil {
				s.nudged = map[string]time.Time{}
			}
			for _, t := range due {
				s.nudged[t.ID] = now
			}
			if len(due) > 0 {
				s.nudged[longTaskLast] = now
			}
			s.mu.Unlock()
			if len(due) > 0 {
				err := s.guide(longTaskNote(due, now), nil)
				s.event("check-in", map[string]any{"why": "long background tasks", "tasks": due, "err": errText(err)})
			}
		}
	}
}

// overdueTasks are the shells and monitors running past longTaskAfter not
// asked about in the last longTaskReask. Once one is due, so is every other
// running past longTaskAfter, so they're asked about in one check-in, not
// one a minute apart; and none is asked about within longTaskAgain of the
// last check-in.
func overdueTasks(tasks []Task, nudged map[string]time.Time, now time.Time) []Task {
	if at, ok := nudged[longTaskLast]; ok && now.Sub(at) < longTaskAgain {
		return nil
	}
	var long []Task
	fresh := false
	for _, t := range tasks {
		if t.StartedAt.IsZero() || now.Sub(t.StartedAt) < longTaskAfter || subagentTask(t.Type) {
			continue
		}
		long = append(long, t)
		if at, ok := nudged[t.ID]; !ok || now.Sub(at) >= longTaskReask {
			fresh = true
		}
	}
	if !fresh {
		return nil
	}
	return long
}

// askable are its background tasks it may be asked about: not a
// subagent's, which looks after its own, nor one known to have ended
// that the agent's list still has, nor a rush watch, which ends itself
// and wakes the agent. Called with mu held.
func (s *server) askable() []Task {
	now := time.Now()
	return slices.DeleteFunc(slices.Clone(s.info.Background), func(t Task) bool {
		return t.Parent != "" || s.notAsked[t.ID] || selfEnding(t, now)
	})
}

// selfEnding is whether t is a running rush watch: it ends at its timeout
// at the latest, so asking about it only buys a turn that says "it's fine".
var selfEnding = func(t Task, now time.Time) bool {
	_, ok := watch.LiveFor(t.ID, now)
	return ok
}

// dontAsk keeps a task out of check-ins. Called with mu held.
func (s *server) dontAsk(id string) {
	if s.notAsked == nil {
		s.notAsked = map[string]bool{}
	}
	s.notAsked[id] = true
}

// taskEnded is whether a task's status says it has ended.
func taskEnded(status string) bool {
	switch status {
	case "completed", "failed", "killed", "stopped", "cancelled":
		return true
	}
	return false
}

// subagentTask is whether a task of this type is an agent doing work.
func subagentTask(typ string) bool {
	switch typ {
	case "local_agent", "remote_agent", "in_process_teammate", "local_workflow":
		return true
	}
	return false
}

// longTaskNote is what the agent is told about them.
func longTaskNote(due []Task, now time.Time) string {
	var b strings.Builder
	b.WriteString("Rush check-in: these background tasks have been running a long time:\n")
	for _, t := range due {
		fmt.Fprintf(&b, "- %s %q (%s), running %s\n", or(t.Type, "task"), short(agentCommand(or(t.Label, t.ID))), t.ID, now.Sub(t.StartedAt).Round(time.Minute))
	}
	b.WriteString("You have the context to judge them: is each still doing something useful, or waiting on something that won't happen? Have a look and decide what to do, then carry on. If they're fine, say so in a line and keep going.")
	return b.String()
}
