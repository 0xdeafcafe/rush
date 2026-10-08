package main

import (
	"bufio"
	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func replayExchanges(t *testing.T, id string) []event.Exchange {
	t.Helper()
	c, err := host.Dial(id)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	var result []event.Exchange
	seen := map[string]bool{}
	if err := awaitLine(c, 5*time.Second, func(ev any) bool {
		if e, ok := ev.(event.Exchange); ok && !seen[e.Direction+e.ID] {
			seen[e.Direction+e.ID] = true
			result = append(result, e)
		}
		_, done := ev.(host.InfoEvent)
		return done
	}); err != nil {
		t.Fatal(err)
	}
	return result
}

func TestAgentSessionSendAndReturnBothTranscripts(t *testing.T) {
	bin := setup(t)
	from := startJSON(t, "--cwd", filepath.Dir(bin), "--binary", bin, "--name", "Reviewer")["id"].(string)
	to := startJSON(t, "--cwd", filepath.Dir(bin), "--binary", bin, "--name", "Builder")["id"].(string)
	t.Setenv("RUSH_SESSION", from)
	text := "Review findings\nPreserve the full detail."
	if out, code := run(t, text, "send", to); code != 0 {
		t.Fatalf("send: %d %s", code, out)
	}
	sent, received := replayExchanges(t, from), replayExchanges(t, to)
	if len(sent) != 1 || len(received) != 1 {
		t.Fatalf("sender %v receiver %v", sent, received)
	}
	a, b := sent[0], received[0]
	if a.ID == "" || a.ID != b.ID || a.Direction != "sent" || b.Direction != "received" || a.Text != text || b.Text != text || b.Sender.Name != "Reviewer" || b.Receiver.Name != "Builder" {
		t.Fatalf("bad correlation: %+v %+v", a, b)
	}
	recordReturn(&a, to, "Complete answer\nSecond line", 0)
	sent, received = replayExchanges(t, from), replayExchanges(t, to)
	if len(sent) != 2 || len(received) != 2 || sent[1].ID != a.ID+"-result" || sent[1].Direction != "received" || received[1].Direction != "sent" || sent[1].Text != "Complete answer\nSecond line" {
		t.Fatalf("return correlation: %+v %+v", sent, received)
	}
}

func TestDeliveredExchangeMirrorFailureStillSucceeds(t *testing.T) {
	bin := setup(t)
	to := startJSON(t, "--cwd", filepath.Dir(bin), "--binary", bin)["id"].(string)
	dir := filepath.Join(host.Root(), "offline-source")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.json"), []byte(`{"id":"offline-source","kind":"codex","name":"Offline reviewer"}`), 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("RUSH_SESSION", "offline-source")
	out, code := run(t, "deliver once", "send", to)
	if code != 0 || !strings.Contains(out, "warning: delivered") {
		t.Fatalf("delivery became retryable failure: %d %s", code, out)
	}
	if got := replayExchanges(t, to); len(got) != 1 || got[0].Text != "deliver once" {
		t.Fatalf("delivery: %+v", got)
	}
}

func TestLegacyHostExchangeFallback(t *testing.T) {
	bin := setup(t)
	source := startJSON(t, "--cwd", filepath.Dir(bin), "--binary", bin)["id"].(string)
	t.Setenv("RUSH_SESSION", source)
	id := "legacy"
	dir := filepath.Join(host.Root(), id)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	// Remove the synthetic own-process info before setup's host cleanup runs.
	defer os.RemoveAll(dir)
	info := host.Info{ID: id, HostPID: os.Getpid(), Proto: 7}
	b, _ := jsonx.Marshal(info)
	if err := os.WriteFile(filepath.Join(dir, "info.json"), b, 0600); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", host.SockPath(id))
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	commands := make(chan map[string]any, 1)
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}

			b, _ := jsonx.Marshal(map[string]any{"type": "agtop_info", "info": info})
			c.Write(append(b, '\n'))
			scanner := bufio.NewScanner(c)
			for scanner.Scan() {
				var op map[string]any
				jsonx.Unmarshal(scanner.Bytes(), &op)
				if op["op"] == "send" {
					commands <- op
					c.Write([]byte(`{"agtop_sent":true,"message":{"role":"user","content":"legacy message"},"type":"user"}` + "\n"))
					c.Close()
					return
				}
			}
			c.Close()
		}
	}()
	out, code := run(t, "legacy message", "send", id)
	if code != 0 || !strings.Contains(out, "older host") {
		t.Fatalf("legacy send: %d %s", code, out)
	}
	select {
	case op := <-commands:
		if op["exchange"] != nil || op["guide"] != true {
			t.Fatalf("legacy command %+v", op)
		}
	case <-time.After(time.Second):
		t.Fatal("no legacy send")
	}
	if got := replayExchanges(t, source); len(got) != 0 {
		t.Fatalf("claimed legacy mirror: %+v", got)
	}
}

func TestExchangeSurvivesHostReplacement(t *testing.T) {
	bin := setup(t)
	started := startJSON(t, "--cwd", filepath.Dir(bin), "--binary", bin)
	id := started["id"].(string)
	e := event.Exchange{ID: "durable", Direction: "sent", Text: "Keep this exchange", Sender: event.Peer{SessionID: id, Kind: "claude"}, Receiver: event.Peer{SessionID: "other", Kind: "kimi"}}
	if err := recordExchangeAt(id, e); err != nil {
		t.Fatal(err)
	}
	cfg, err := host.ReadConfig(id)
	if err != nil {
		t.Fatal(err)
	}
	info, _ := host.ReadInfo(id)
	if err := syscall.Kill(info.HostPID, syscall.SIGTERM); err != nil {
		t.Fatal(err)
	}
	waitFor(t, "old host exit", func() bool { return !host.Alive(info.HostPID) })
	cfg.Resume = true
	cfg.Prompt = ""
	cfg.PromptExchange = nil
	if _, err := host.Spawn(cfg); err != nil {
		t.Fatal(err)
	}
	got := replayExchanges(t, id)
	if len(got) != 1 || got[0].ID != "durable" || !got[0].Archived || got[0].Text != e.Text {
		t.Fatalf("lost exchange after host replacement: %+v", got)
	}
}
