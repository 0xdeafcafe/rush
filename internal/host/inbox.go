package host

import (
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// A session's inbox holds messages for its running subagents, a file per
// subagent by its id, taken by the agent's hook (rush inbox) at that
// subagent's next tool call.

func inboxDir(id string) string { return filepath.Join(dir(id), "inbox") }

// inboxHook is the hook command for session id: only a waiting message
// starts rush, so the shell's glob is all most tool calls cost.
func inboxHook(id string) string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	if r, err := filepath.EvalSymlinks(exe); err == nil {
		exe = r
	}
	d := quote(inboxDir(id))
	return "set -- " + d + "/*; [ -e \"$1\" ] && exec " + quote(exe) + " inbox " + d + "; exit 0"
}

// takesInbox is whether kind's subagents can be sent messages straight.
func takesInbox(kind string) bool {
	_, ok := agent.As[agent.SubagentInbox](agent.HarnessOf(agent.Kind(kind)))
	return ok
}

// tell leaves text for subagent sub, after anything already waiting.
func tell(id, sub, text string) error {
	if sub == "" || strings.ContainsAny(sub, `/\.`) {
		return errors.New("no such subagent")
	}
	if err := os.MkdirAll(inboxDir(id), 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(filepath.Join(inboxDir(id), sub), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	_, err = f.WriteString(text + "\n\n")
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	return err
}

// unread gives the main session what subagent sub ended without reading,
// so a message sent it isn't lost. Called with mu held.
func (s *server) unread(sub string) {
	if !s.info.Inbox || sub == "" || strings.ContainsAny(sub, `/\.`) {
		return
	}
	p := filepath.Join(inboxDir(s.cfg.ID), sub)
	b, err := os.ReadFile(p)
	if err != nil {
		return
	}
	_ = os.Remove(p)
	if strings.TrimSpace(string(b)) == "" {
		return
	}
	qi := queueImages(&s.info)
	qe := queueExchanges(&s.info)
	s.info.Queue = append(s.info.Queue, "I sent this to your subagent "+sub+", but it finished before it read it:\n\n"+strings.TrimSpace(string(b)))
	s.info.QueueImages = trimImages(append(qi, nil))
	s.info.QueueExchanges = trimExchanges(append(qe, nil))
	if s.info.State == "idle" && !s.info.QueueHeld {
		s.sendQueue()
		return
	}
	s.publish()
}

// MainInbox is the inbox file for the main session itself: slipped into
// its turn at its next tool call, without stopping it.
const MainInbox = "main"

// TellNote heads what the hook hands over. The transcript keeps it, so a
// view can show what follows it as your message.
const TellNote = "The user sent you this message directly while you work. Take it into account, then carry on:\n\n"

// Inbox is the hook: given a tool call's hook input on in, it writes to out
// what's waiting in dir for the subagent that made it, and takes it.
func Inbox(dir string, in io.Reader, out io.Writer) error {
	var h struct {
		Event   string `json:"hook_event_name"`
		AgentID string `json:"agent_id"`
	}
	if err := jsonx.Decode(in, &h); err != nil || strings.ContainsAny(h.AgentID, `/\.`) {
		return err
	}
	if h.AgentID == "" {
		h.AgentID = MainInbox // the main session's own call
	}
	// Taken by renaming, so two calls at once can't both hand it over.
	p, taking := filepath.Join(dir, h.AgentID), filepath.Join(dir, "."+h.AgentID+".taking")
	if os.Rename(p, taking) != nil {
		return nil
	}
	b, err := os.ReadFile(taking)
	_ = os.Remove(taking)
	if err != nil || strings.TrimSpace(string(b)) == "" {
		return err
	}
	msg := TellNote + strings.TrimSpace(string(b))
	b, err = jsonx.Marshal(map[string]any{"hookSpecificOutput": map[string]any{"hookEventName": h.Event, "additionalContext": msg}})
	if err != nil {
		return err
	}
	_, err = out.Write(b)
	return err
}
