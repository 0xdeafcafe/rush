package ui

import (
	"fmt"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
)

// What rush does to a session running outside rush mode goes through its
// agent's adapter (agent.Stopper and the rest): rush knows no agent's
// program itself.

// stopOutside ends a's process and keeps its conversation, the agent's own
// way; one whose agent has none gets a SIGTERM. A rush session stops with
// the agents it spawned, as ctrl+x stops it: none works on unseen.
func stopOutside(a *fleet.Agent) error {
	if a.Rush {
		return host.StopAll(a.ID)
	}
	if s, ok := agent.As[agent.Stopper](agent.Kind(a.Kind)); ok {
		return s.Stop(a.Acct, a.ID, a.PID)
	}
	if a.PID != 0 {
		return actions.Terminate(a.PID)
	}
	return fmt.Errorf("%s can't stop %s", harnessName(a.Kind), a.DisplayName)
}

// removeOutside deletes a session its agent keeps.
func removeOutside(a *fleet.Agent) error {
	if r, ok := agent.As[agent.Remover](agent.Kind(a.Kind)); ok {
		return r.Remove(a.Acct, a.ID)
	}
	return fmt.Errorf("%s keeps its sessions itself: rush can't delete them", harnessName(a.Kind))
}

// replyOutside sends text to a session of agent k's background service;
// one it has let go of is agent.ErrGone.
func replyOutside(k agent.Kind, p agent.Profile, id, text string) error {
	if r, ok := agent.As[agent.Replier](k); ok {
		return r.Reply(p, id, text)
	}
	return fmt.Errorf("%s takes no messages from outside it", harnessName(string(k)))
}
