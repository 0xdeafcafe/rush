package agtools

import (
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
)

// The reminder tools let a session have rush send it a message at a time
// it picks (remind) and see or cancel what it set (reminders): rush keeps
// the clock, so the agent needn't stay awake or loop to come back later.

// The reminder tools' own names.
const (
	RemindTool    = "remind"
	RemindersTool = "reminders"
)

// RemindInput is what a session sends remind.
type RemindInput struct {
	At   string `json:"at"`
	Text string `json:"text"`
}

// RemindersInput is what a session sends reminders.
type RemindersInput struct {
	Cancel string `json:"cancel,omitempty"`
}

// Reminders keeps one session's reminders; the agent tools' Agents offer
// it when they can.
type Reminders interface {
	Remind(in RemindInput) (string, error)
	Reminders(in RemindersInput) (string, error)
}

var remindTools = []tool{{
	Name: RemindTool,
	Description: "Have rush send you a message at a time you pick, as a task due then: check a deploy in 20 minutes, " +
		"follow up on a review this afternoon, look at a long job's results tomorrow at 9. rush keeps the clock and " +
		"wakes you with the text when it falls due (queued behind your turn if you are at work), so end your turn " +
		"instead of waiting, sleeping or looping. Write the text as a note to your future self with all it needs: " +
		"what to do and where, since by then you may remember little else. Returns the reminder's id.",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"at": map[string]any{"type": "string", "description": `When it's due: a wait ("20m", "2h", "1h30m"), a time of day ` +
				`in the user's zone ("15:30", "3pm", "tomorrow 9:00"; the next one to come), or a date and time ("2026-11-02 09:00", RFC 3339).`},
			"text": map[string]any{"type": "string", "description": "What to do when it's due, written for you to act on then."},
		},
		"required":             []string{"at", "text"},
		"additionalProperties": false,
	},
}, {
	Name:        RemindersTool,
	Description: "List the reminders you set with remind, soonest first, or cancel one by its id (\"all\" cancels every one).",
	InputSchema: map[string]any{
		"type": "object",
		"properties": map[string]any{
			"cancel": map[string]any{"type": "string", "description": `A reminder's id to cancel, or "all".`},
		},
		"additionalProperties": false,
	},
}}

func remindList(ag Agents) []tool {
	if _, ok := ag.(Reminders); ok {
		return remindTools
	}
	return nil
}

// callRemind runs a reminder tool; false when name isn't one, or ag
// keeps no reminders.
func callRemind(ag Agents, name string, args jsontext.Value) (map[string]any, bool) {
	r, ok := ag.(Reminders)
	if !ok {
		return nil, false
	}
	var (
		out string
		err error
	)
	switch name {
	case RemindTool:
		var in RemindInput
		if err := jsonx.Unmarshal(args, &in); err != nil {
			return result("Its input: "+err.Error(), true), true
		}
		out, err = r.Remind(in)
	case RemindersTool:
		var in RemindersInput
		if len(args) > 0 {
			if err := jsonx.Unmarshal(args, &in); err != nil {
				return result("Its input: "+err.Error(), true), true
			}
		}
		out, err = r.Reminders(in)
	default:
		return nil, false
	}
	if err != nil {
		return result(err.Error(), true), true
	}
	return result(out, false), true
}
