package host

import (
	"fmt"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/remind"
)

// Remind has rush send the session in.Text at in.At (agtools.Reminders).
func (sa subagents) Remind(in agtools.RemindInput) (string, error) {
	now := time.Now()
	due, err := remind.Parse(in.At, now)
	if err != nil {
		return "", err
	}
	r, err := remind.Add(sa.parent, in.Text, due, now)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("Set: reminder %s, due %s (%s). rush will send you its text then; "+
		"end your turn rather than wait for it.", r.ID, r.Due.Local().Format("Mon 2 Jan 15:04 MST"), remind.Until(r.Due, now)), nil
}

// Reminders lists the session's reminders, or cancels one first.
func (sa subagents) Reminders(in agtools.RemindersInput) (string, error) {
	var done string
	switch id := strings.TrimSpace(in.Cancel); id {
	case "":
	case "all":
		if err := remind.CancelAll(sa.parent); err != nil {
			return "", err
		}
		done = "Cancelled them all.\n"
	default:
		if err := remind.Cancel(sa.parent, id); err != nil {
			return "", err
		}
		done = "Cancelled " + id + ".\n"
	}
	now := time.Now()
	list := remind.List(sa.parent)
	if len(list) == 0 {
		return done + "No reminders set.", nil
	}
	lines := make([]string, 0, len(list))
	for _, r := range list {
		lines = append(lines, remind.Line(r, now))
	}
	return done + strings.Join(lines, "\n"), nil
}
