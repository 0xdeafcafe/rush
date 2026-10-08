package ui

import (
	"errors"
	"github.com/0xdeafcafe/rush/internal/host"
	"testing"
	"time"
)

func TestAwaitCarried(t *testing.T) {
	if err := awaitCarried(func() (bool, error) { return true, nil }, time.Second); err != nil {
		t.Fatal(err)
	}
	want := errors.New("login failed")
	if err := awaitCarried(func() (bool, error) { return false, want }, time.Second); !errors.Is(err, want) {
		t.Fatal(err)
	}
	if err := awaitCarried(func() (bool, error) { return false, nil }, time.Millisecond); err == nil {
		t.Fatal("uninitialized destination accepted")
	}
}

func TestHandoffQuiet(t *testing.T) {
	for _, info := range []host.Info{
		{State: "working"}, {State: "blocked"}, {State: "idle", Queue: []string{"do not lose this"}},
		{State: "idle", Background: []host.Task{{ID: "running"}}}, {State: "idle", Needs: "approval"},
	} {
		if handoffQuiet(info) {
			t.Fatalf("unsafe source: %+v", info)
		}
	}
	for _, state := range []string{"idle", "stopped"} {
		if !handoffQuiet(host.Info{State: state}) {
			t.Fatal(state)
		}
	}
}

func TestOllamaModelSwitchRestarts(t *testing.T) {
	from := startOver{kind: "ollama", model: "small"}
	to := from
	to.model = "large"
	if inPlace(from, to) {
		t.Fatal("Ollama model change retained old startup settings")
	}
	if !inPlace(from, from) {
		t.Fatal("unchanged Ollama model needlessly restarts")
	}
	from.kind, to.kind = "codex", "codex"
	if !inPlace(from, to) {
		t.Fatal("Codex model switch lost in-place support")
	}
}

func TestBusySwitchPreservesInstructionDraft(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	c.sess.Info.State = "working"
	draft := "#agent codex keep this instruction"
	c.input = []rune(draft)
	cmd := m.switchSessionMessage(c, startOver{kind: "codex"}, "keep this instruction")
	if cmd == nil {
		t.Fatal("busy command should restore its draft")
	}
	c.input = nil // sendPane clears accepted command text before its result returns
	apply, ok := cmd().(applyMsg)
	if !ok {
		t.Fatal("busy switch tried to start another session")
	}
	apply(m)
	if string(c.input) != draft {
		t.Fatalf("lost draft: %q", c.input)
	}
	c.input = []rune("a newer draft")
	apply(m)
	if string(c.input) != "a newer draft" {
		t.Fatal("overwrote newer input")
	}
}
