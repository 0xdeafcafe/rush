// Package remind keeps the reminders agents set: a message rush sends a
// session at a time the agent chose, so it can come back to something
// later without staying awake to watch the clock. Each reminder is a file
// under Dir, and a small detached rush process (rush remind run) waits for
// it and sends it with the session's host, waking the session if it
// sleeps or queuing behind a turn at work. Nothing polls: the process
// sleeps until the reminder is due.
package remind

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/selfexe"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A Reminder is one message a session asked rush to send it at Due.
type Reminder struct {
	ID      string    `json:"id"`
	Session string    `json:"session"`
	Text    string    `json:"text"`
	Due     time.Time `json:"due"`
	Created time.Time `json:"created"`
}

// Longest is how far ahead a reminder may be set.
const Longest = 90 * 24 * time.Hour

// Dir is where reminders and their waiters' locks and logs are kept.
func Dir() string { return filepath.Join(state.Dir(), "reminders") }

func path(id string) string { return filepath.Join(Dir(), id) }

// Add saves a reminder for session, due at due, and starts its waiter.
func Add(session, text string, due, now time.Time) (Reminder, error) {
	text = strings.TrimSpace(text)
	switch {
	case session == "":
		return Reminder{}, errors.New("no session to remind")
	case text == "":
		return Reminder{}, errors.New("a reminder needs its text")
	case due.After(now.Add(Longest)):
		return Reminder{}, fmt.Errorf("that is more than %d days away", int(Longest.Hours()/24))
	}
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	r := Reminder{ID: hex.EncodeToString(b), Session: session, Text: text, Due: due, Created: now}
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		return Reminder{}, err
	}
	raw, _ := jsonx.Marshal(r)
	tmp := path(r.ID) + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o600); err != nil {
		return Reminder{}, err
	}
	if err := os.Rename(tmp, path(r.ID)+".json"); err != nil {
		return Reminder{}, err
	}
	return r, Start(r.ID)
}

// Read is the reminder saved as id.
func Read(id string) (Reminder, error) {
	var r Reminder
	b, err := os.ReadFile(path(id) + ".json")
	if err != nil {
		return r, err
	}
	return r, jsonx.Unmarshal(b, &r)
}

// List is session's reminders, soonest first; every session's, "".
func List(session string) []Reminder {
	files, _ := filepath.Glob(filepath.Join(Dir(), "*.json"))
	var out []Reminder
	for _, f := range files {
		r, err := Read(strings.TrimSuffix(filepath.Base(f), ".json"))
		if err == nil && (session == "" || r.Session == session) {
			out = append(out, r)
		}
	}
	slices.SortFunc(out, func(a, b Reminder) int { return a.Due.Compare(b.Due) })
	return out
}

// Cancel drops session's reminder id; its waiter sees it gone and ends.
func Cancel(session, id string) error {
	r, err := Read(id)
	if err != nil || r.Session != session {
		return fmt.Errorf("no reminder %s", id)
	}
	return remove(id)
}

// CancelAll drops every reminder session has.
func CancelAll(session string) error {
	var err error
	for _, r := range List(session) {
		err = errors.Join(err, remove(r.ID))
	}
	return err
}

func remove(id string) error {
	err := os.Remove(path(id) + ".json")
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// held is whether a waiter holds id's lock.
func held(id string) bool {
	f, err := os.OpenFile(path(id)+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return false
	}
	defer f.Close()
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		return true
	}
	_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
	return false
}

// Start has a detached waiter of its own wait for reminder id, unless one
// already does.
func Start(id string) error { return starting(id) }

// starting is Start's way of starting a waiter; tests start none.
var starting = startWaiter

func startWaiter(id string) error {
	if held(id) {
		return nil
	}
	exe, err := selfexe.Path()
	if err != nil {
		return err
	}
	log, err := os.OpenFile(path(id)+".log", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer log.Close()
	cmd := exec.Command(exe, "remind", "run", id)
	cmd.Stdout, cmd.Stderr = log, log
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}

// Resume starts a waiter for each reminder that has none, as after a
// restart of the machine, when every waiter ended with it. A reminder that
// fell due meanwhile is sent at once.
func Resume() {
	for _, r := range List("") {
		_ = Start(r.ID)
	}
}

// Hold takes reminder id's lock until release is called; false when
// another waiter has it.
func Hold(id string) (release func(), ok bool) {
	f, err := os.OpenFile(path(id)+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, false
	}
	if syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		f.Close()
		return nil, false
	}
	return func() { f.Close() }, true
}

// A Waiter waits for one reminder and sends it. Its clock and its way of
// sending are its own, for tests.
type Waiter struct {
	Now   func() time.Time
	Sleep func(time.Duration)
	// Deliver sends the reminder's message to its session.
	Deliver func(r Reminder, text string) error
	// Gone is whether the session no longer exists, so nothing is sent.
	Gone func(session string) bool
}

// slice is the longest a waiter sleeps between looks: short enough that
// a cancelled reminder's waiter ends soon, and that a machine waking from
// sleep past the due time sends it within a minute.
const slice = time.Minute

// tries is how many times a waiter tries to send before it gives up.
const tries = 5

// Run waits until reminder id is due, then sends it and drops it. It ends
// early when the reminder is cancelled or its session is gone.
func (w Waiter) Run(id string) error {
	for {
		r, err := Read(id)
		if err != nil {
			tidy(id, true)
			return nil // cancelled
		}
		if w.Gone(r.Session) {
			tidy(id, true)
			return remove(id)
		}
		left := r.Due.Sub(w.Now())
		if left > 0 {
			w.Sleep(min(left, slice))
			continue
		}
		for i := range tries {
			if err = w.Deliver(r, Message(r, w.Now())); err == nil {
				break
			}
			fmt.Fprintf(os.Stderr, "%s rush remind: sending %s: %v\n", w.Now().Format(time.RFC3339), id, err)
			if i < tries-1 {
				w.Sleep(time.Duration(i+1) * 30 * time.Second)
			}
		}
		tidy(id, err == nil) // a failure's log is kept
		return errors.Join(err, remove(id))
	}
}

// tidy removes the lock and, when log, the log a waiter for id leaves.
func tidy(id string, log bool) {
	_ = os.Remove(path(id) + ".lock")
	if log {
		_ = os.Remove(path(id) + ".log")
	}
}

// Message is what the session is sent when r falls due at now.
func Message(r Reminder, now time.Time) string {
	late := ""
	if d := now.Sub(r.Due); d > 2*time.Minute {
		late = ", " + span(d) + " late (the machine was asleep or off)"
	}
	return fmt.Sprintf("<rush-reminder id=%q set=%q due=%q>\nA reminder you set%s:\n\n%s\n</rush-reminder>",
		r.ID, r.Created.Local().Format(time.DateTime), r.Due.Local().Format(time.DateTime), late, r.Text)
}

// Line is r as one line, for lists.
func Line(r Reminder, now time.Time) string {
	return fmt.Sprintf("%s  due %s (%s)  %s", r.ID, r.Due.Local().Format("Mon 2 Jan 15:04"), Until(r.Due, now), firstLine(r.Text))
}

func firstLine(s string) string {
	s, _, _ = strings.Cut(s, "\n")
	if r := []rune(s); len(r) > 80 {
		return string(r[:79]) + "…"
	}
	return s
}

// Until says how long until t, in words: "in 25m", "now".
func Until(t, now time.Time) string {
	if t.Sub(now).Round(time.Minute) <= 0 {
		return "now"
	}
	return "in " + span(t.Sub(now))
}

// span is d in words, to the minute: "25m", "1h30m", "3d".
func span(d time.Duration) string {
	d = d.Round(time.Minute)
	h, m := int(d.Hours()), int(d.Minutes())%60
	switch {
	case h >= 48:
		return strconv.Itoa(h/24) + "d"
	case h > 0 && m > 0:
		return fmt.Sprintf("%dh%dm", h, m)
	case h > 0:
		return strconv.Itoa(h) + "h"
	}
	return strconv.Itoa(m) + "m"
}

// Parse reads when a reminder is due, relative to now and in its zone:
// a wait ("25m", "in 2h", "1h30m"), a time of day ("15:30", "3pm",
// "3:30pm", "tomorrow 9:00"; today's, or tomorrow's once it has passed),
// or a date and time ("2026-11-02 09:00", RFC 3339).
func Parse(s string, now time.Time) (time.Time, error) {
	raw := strings.TrimSpace(s)
	s = strings.ToLower(raw)
	if d, err := time.ParseDuration(strings.TrimSpace(strings.TrimPrefix(s, "in "))); err == nil {
		if d <= 0 {
			return time.Time{}, fmt.Errorf("%q is not in the future", s)
		}
		return now.Add(d), nil
	}
	for _, layout := range []string{time.RFC3339, "2006-01-02 15:04", "2006-01-02T15:04", "2006-01-02 15:04:05", "2006-01-02T15:04:05"} {
		if t, err := time.ParseInLocation(layout, raw, now.Location()); err == nil {
			if !t.After(now) {
				return time.Time{}, fmt.Errorf("%s has passed", t.Format(time.DateTime))
			}
			return t, nil
		}
	}
	day := 0
	if rest, ok := strings.CutPrefix(s, "tomorrow"); ok {
		day, s = 1, strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(rest), "at "))
	}
	for _, layout := range []string{"15:04", "3pm", "3:04pm", "3 pm", "3:04 pm"} {
		c, err := time.Parse(layout, s)
		if err != nil {
			continue
		}
		t := time.Date(now.Year(), now.Month(), now.Day()+day, c.Hour(), c.Minute(), 0, 0, now.Location())
		if !t.After(now) {
			t = t.AddDate(0, 0, 1)
		}
		return t, nil
	}
	return time.Time{}, fmt.Errorf(`can't read %q as a time: give a wait ("25m", "2h"), a time of day ("15:30", "3pm", "tomorrow 9:00") or a date and time ("2026-11-02 09:00")`, raw)
}
