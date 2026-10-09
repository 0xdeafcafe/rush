package host

import (
	"fmt"
	"slices"
	"strings"
	"time"
)

// A background task can be left running long after it's any use: a
// monitor waiting on what never comes, a loop polling a queue that moved
// on. Rush never stops one; it asks the agent to look, since it knows
// what each was for.
const (
	longTaskAfter = 30 * time.Minute // a background task running this long is asked about
	longTaskAgain = time.Hour        // and again this often while it runs on
)

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
			if s.conn != nil && !s.info.Sleeping && s.info.State != "blocked" && len(s.pending) == 0 && s.info.Limit == nil {
				due = slices.DeleteFunc(overdueTasks(s.info.Background, s.nudged, now), func(t Task) bool { return slices.Contains(ended, t.ID) })
			}
			if len(due) > 0 && s.nudged == nil {
				s.nudged = map[string]time.Time{}
			}
			for _, t := range due {
				s.nudged[t.ID] = now
			}
			s.mu.Unlock()
			if len(due) > 0 {
				_ = s.guide(longTaskNote(due, now), nil)
			}
		}
	}
}

// overdueTasks are the tasks running past longTaskAfter not asked about in
// the last longTaskAgain.
func overdueTasks(tasks []Task, nudged map[string]time.Time, now time.Time) []Task {
	var out []Task
	for _, t := range tasks {
		if t.StartedAt.IsZero() || now.Sub(t.StartedAt) < longTaskAfter {
			continue
		}
		if at, ok := nudged[t.ID]; ok && now.Sub(at) < longTaskAgain {
			continue
		}
		out = append(out, t)
	}
	return out
}

// longTaskNote is what the agent is told about them.
func longTaskNote(due []Task, now time.Time) string {
	var b strings.Builder
	b.WriteString("Rush check-in: these background tasks have been running a long time:\n")
	for _, t := range due {
		fmt.Fprintf(&b, "- %s %q (%s), running %s\n", or(t.Type, "task"), or(t.Label, t.ID), t.ID, now.Sub(t.StartedAt).Round(time.Minute))
	}
	b.WriteString("You have the context to judge them: is each still doing something useful, or waiting on something that won't happen? Have a look and decide what to do, then carry on. If they're fine, say so in a line and keep going.")
	return b.String()
}
