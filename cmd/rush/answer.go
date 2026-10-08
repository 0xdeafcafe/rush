package main

import (
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// skippedQuestion is what the agent is told when a question is declined.
const skippedQuestion = "The user skipped the question; carry on with your best judgement."

// sessionAnswer settles what a session is waiting on, as the view's card
// does: the text on stdin answers its question (the first one, when it asks
// several), and a tool call it asks permission for is allowed. --deny
// declines either; --request names the request (its id, or the tool call's),
// so an answer meant for one that was settled meanwhile doesn't land on the
// next.
func sessionAnswer(args []string, stdin io.Reader, stdout io.Writer) error {
	fs := newFlags("answer")
	var (
		deny    bool
		request string
	)
	fs.BoolVar(&deny, "deny", false, "")
	fs.StringVar(&request, "request", "", "")
	id, err := idAndFlags(fs, args)
	if err != nil {
		return err
	}
	if !sessionExists(id) {
		return fmt.Errorf("session %s %w", id, errNotFound)
	}
	if info, err := host.ReadInfo(id); err != nil || !host.Alive(info.HostPID) {
		return fmt.Errorf("session %s is not running", id)
	}
	b, err := io.ReadAll(stdin)
	if err != nil {
		return err
	}
	text := strings.TrimSpace(string(b))
	c, err := host.Dial(id)
	if err != nil {
		return err
	}
	defer c.Close()
	w, err := pendingOf(c, 10*time.Second)
	if err != nil {
		return err
	}
	r := w.last()
	switch {
	case r == nil:
		return fmt.Errorf("session %s is not waiting on an answer", id)
	case request != "" && r.ID != request && r.CallID != request:
		return fmt.Errorf("session %s is not waiting on %s any more", id, request)
	}
	switch {
	case deny && r.Q != nil:
		err = c.Deny(r.ID, skippedQuestion, false)
	case deny:
		err = c.Deny(r.ID, text, false)
	case r.Q != nil:
		if text == "" {
			return errors.New("nothing to answer: write the answer on stdin")
		}
		if len(r.Q.Asks) == 0 {
			return fmt.Errorf("session %s asked a question with nothing in it", id)
		}
		err = c.Allow(r.ID, host.AnswerInput(r.Q, map[string]string{r.Q.Asks[0].Text: text}), false)
	default:
		err = c.Allow(r.ID, nil, false)
	}
	if err != nil {
		return err
	}
	var failed error
	_ = awaitLine(c, 5*time.Second, func(ev any) bool {
		switch e := ev.(type) {
		case host.ErrorEvent:
			failed = errors.New(e.Error)
			return true
		case host.Answered:
			return e.ID == r.ID
		}
		return false
	})
	if failed != nil {
		return failed
	}
	fmt.Fprintf(stdout, "answered %s\n", id)
	return nil
}

// asking is a request a session waits on: a question, or a tool call to
// allow.
type asking struct {
	ID     string
	CallID string // the tool call it is about
	Q      *event.Question
}

// waits are what a replay asked and hasn't yet settled, in order.
type waits struct {
	open  map[string]*asking
	order []string
}

func (w *waits) take(ev any) {
	switch ev := ev.(type) {
	case event.Approval:
		w.open[ev.ID] = &asking{ID: ev.ID, CallID: ev.Call.ID}
		w.order = append(w.order, ev.ID)
	case event.Question:
		q := ev
		w.open[ev.ID] = &asking{ID: ev.ID, CallID: ev.CallID, Q: &q}
		w.order = append(w.order, ev.ID)
	case event.ApprovalCancelled:
		delete(w.open, ev.ID)
	case host.Answered:
		delete(w.open, ev.ID)
	}
}

func (w *waits) last() *asking {
	for _, id := range slices.Backward(w.order) {
		if r := w.open[id]; r != nil {
			return r
		}
	}
	return nil
}

// pendingOf reads the host's replay, which ends with its info, for the
// requests still open.
func pendingOf(c *host.Client, d time.Duration) (*waits, error) {
	w := &waits{open: map[string]*asking{}}
	var dec host.Decoder
	timeout := time.After(d)
	for {
		select {
		case l, ok := <-c.Lines:
			if !ok {
				return nil, errors.New("the host closed the connection")
			}
			evs, _ := dec.Decode(l)
			for _, ev := range evs {
				if _, ok := ev.(host.InfoEvent); ok {
					return w, nil
				}
				w.take(ev)
			}
		case <-timeout:
			return nil, errors.New("the host did not answer in time")
		}
	}
}
