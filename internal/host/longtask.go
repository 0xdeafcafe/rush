package host

import (
	"fmt"
	"os"
	"path/filepath"
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
// check-in only buys a turn that says "it's fine". A task is judged by
// its output, not its age: one still writing is getting on with it, and
// only one quiet a long time is asked about.
const (
	longTaskAfter = 30 * time.Minute // a background task quiet this long is asked about
	longTaskAgain = time.Hour        // and a session is asked at most this often
	// One running this long is asked about however much it writes: a loop
	// polling for what never comes can print all the while.
	longTaskAnyway = 4 * time.Hour
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
			sid := s.info.SessionID
			ids := taskIDs(s.info.Background)
			s.mu.Unlock()
			wrote := lastOutput(s.taskDirs(sid), ids)
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
				due = slices.DeleteFunc(overdueTasks(s.askable(), wrote, s.nudged, now), func(t Task) bool { return slices.Contains(ended, t.ID) })
			}
			if held != "" && len(overdueTasks(s.askable(), wrote, s.nudged, now)) == 0 {
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
				cut, err := s.checkIn(longTaskNote(due, wrote, now))
				s.event("check-in", map[string]any{"why": "long background tasks", "tasks": due, "interrupted": cut, "err": errText(err)})
			}
		}
	}
}

// checkIn gives the agent rush's note: into the turn under way where it
// can take one, else stopping the turn for it (cut), since queued it would
// wait on the very turn it asks about.
func (s *server) checkIn(note string) (cut bool, err error) {
	s.mu.Lock()
	cut = s.cutsIn() && !takesMidTurn(s.cfg.Kind)
	s.mu.Unlock()
	if cut {
		return true, s.send(note, nil, true)
	}
	return false, s.guide(note, nil)
}

// overdueTasks are the shells and monitors quiet past longTaskAfter (or
// running past longTaskAnyway) not asked about in the last longTaskReask.
// wrote is when each last wrote output, where that's known. Once one is
// due, so is every other quiet that long, so they're asked about in one
// check-in, not one a minute apart; and none is asked about within
// longTaskAgain of the last check-in.
func overdueTasks(tasks []Task, wrote map[string]time.Time, nudged map[string]time.Time, now time.Time) []Task {
	if at, ok := nudged[longTaskLast]; ok && now.Sub(at) < longTaskAgain {
		return nil
	}
	var long []Task
	fresh := false
	for _, t := range tasks {
		if t.StartedAt.IsZero() || subagentTask(t.Type) {
			continue
		}
		if now.Sub(quietSince(t, wrote)) < longTaskAfter && now.Sub(t.StartedAt) < longTaskAnyway {
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

// quietSince is when t last showed it was doing something: its last
// output, or its start if it has written none since.
func quietSince(t Task, wrote map[string]time.Time) time.Time {
	if w := wrote[t.ID]; w.After(t.StartedAt) {
		return w
	}
	return t.StartedAt
}

// taskDirs are the folders Claude Code keeps the session's background
// task output in: one for each project folder it has worked in.
func (s *server) taskDirs(sid string) []string {
	if sid == "" {
		return nil
	}
	found, _ := filepath.Glob(filepath.Join(TempDir(s.cfg.ID), "claude-*", "*", sid, "tasks"))
	return found
}

func taskIDs(tasks []Task) []string {
	ids := make([]string, len(tasks))
	for i, t := range tasks {
		ids[i] = t.ID
	}
	return ids
}

// lastOutput is when each task last wrote to its output file in dirs.
func lastOutput(dirs []string, ids []string) map[string]time.Time {
	wrote := map[string]time.Time{}
	for _, dir := range dirs {
		for _, id := range ids {
			if fi, err := os.Stat(filepath.Join(dir, id+".output")); err == nil && fi.ModTime().After(wrote[id]) {
				wrote[id] = fi.ModTime()
			}
		}
	}
	return wrote
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
func longTaskNote(due []Task, wrote map[string]time.Time, now time.Time) string {
	var b strings.Builder
	b.WriteString("Rush check-in: these background tasks have gone quiet, or been running a long time:\n")
	for _, t := range due {
		fmt.Fprintf(&b, "- %s %q (%s), running %s", or(t.Type, "task"), short(agentCommand(or(t.Label, t.ID))), t.ID, now.Sub(t.StartedAt).Round(time.Minute))
		if q := quietSince(t, wrote); q.After(t.StartedAt) {
			fmt.Fprintf(&b, ", last wrote %s ago", now.Sub(q).Round(time.Minute))
		} else if _, ok := wrote[t.ID]; ok {
			b.WriteString(", nothing written yet")
		}
		b.WriteString("\n")
	}
	b.WriteString("You have the context to judge them: is each still doing something useful, or waiting on something that won't happen? Have a look and decide what to do, then carry on. If they're fine, say so in a line and keep going.")
	return b.String()
}
