package ui

import (
	"bufio"
	tea "charm.land/bubbletea/v2"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestSleepingHostPreservesPaneAndNeverWakesOnView(t *testing.T) {
	m, _ := benchModel(120, 40)
	c := m.host
	a := m.focused()
	a.Rush = true
	c.input = []rune("keep this draft")
	c.back = 4
	c.scroll = 9
	c.sel = "t2"
	c.open["t2"] = true
	original := c.sess
	c.sess.Info.Sleeping = true
	c.sess.Info.HostPID = 4321
	a.PID = 4321
	m.onHostLines(hostLinesMsg{owner: c, key: c.key, closed: true})
	if m.host != c || c.sess != original || !c.sleeping || c.client != nil || c.scroll != 9 || c.sel != "t2" || string(c.input) != "keep this draft" || c.back != 4 {
		t.Fatal("sleep detached the visible conversation or composer")
	}
	for _, pid := range []int{4321, 0, 0, 4321} {
		a.PID = pid
		if cmd := m.syncHost(); cmd != nil {
			t.Fatal("viewing a sleeping host scheduled reconnect/wake")
		}
	}
	a.PID = 9876
	if cmd := m.syncHost(); cmd == nil || m.host != c {
		t.Fatal("externally woken process should reconnect while preserving old view")
	}
	if cmd := m.syncHost(); cmd != nil {
		t.Fatal("duplicate read-only reconnect scheduled")
	}
}

// The fake host speaks only the UI socket protocol. It never launches an
// adapter or real model; the current test PID satisfies Ensure's alive check.
func sleepingSocket(t *testing.T, id string) (*atomic.Int32, host.Info, <-chan string) {
	t.Helper()
	home, err := os.MkdirTemp("/tmp", "rush-sleep-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(home) })
	t.Setenv("RUSH_HOME", home)
	dir := filepath.Dir(host.SockPath(id))
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	info := host.Info{ID: id, Kind: "claude", State: "idle", HostPID: os.Getpid(), StartedAt: time.Now()}
	write := func(name string, v any) {
		b, e := jsonx.Marshal(v)
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(dir, name), b, 0600); e != nil {
			t.Fatal(e)
		}
	}
	write("config.json", host.Config{ID: id, Kind: "claude"})
	write("info.json", info)
	listener, err := net.Listen("unix", host.SockPath(id))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	count := &atomic.Int32{}
	controls := make(chan string, 8)
	serve := func(conn net.Conn) {
		defer conn.Close()
		_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
		infoLine, _ := jsonx.Marshal(map[string]any{"type": "agtop_info", "info": info})
		scanner := bufio.NewScanner(conn)
		for scanner.Scan() {
			var op struct {
				Op   string `json:"op"`
				Text string `json:"text"`
			}
			if jsonx.Unmarshal(scanner.Bytes(), &op) != nil {
				continue
			}
			if op.Op != "hello" && op.Op != "send" {
				controls <- op.Op
			}
			switch op.Op {
			case "hello":
				_, _ = conn.Write([]byte(`{"agtop_sent":true,"message":{"content":"cached turn","role":"user"},"type":"user"}` + "\n" + `{"type":"result","subtype":"success"}` + "\n"))
				_, _ = conn.Write(append(infoLine, '\n'))
			case "send":
				count.Add(1)
				line, _ := jsonx.Marshal(map[string]any{"agtop_sent": true, "type": "user", "message": map[string]any{"content": op.Text, "role": "user"}})
				_, _ = conn.Write(append(line, '\n'))
				working := info
				working.State = "working"
				activeLine, _ := jsonx.Marshal(map[string]any{"type": "agtop_info", "info": working})
				_, _ = conn.Write(append(activeLine, '\n'))
			}
		}
	}
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			go serve(conn)
		}
	}()
	return count, info, controls
}

func TestSleepingSendWakesOnceAndReplacesReplayWithoutDuplicates(t *testing.T) {
	sends, _, _ := sleepingSocket(t, "sleep-test")
	m, c := draftModel()
	m.w, m.h = 120, 40
	a := m.agentByKey(c.key)
	a.Rush = true
	a.ID = "sleep-test"
	a.Kind = "claude"
	c.id = a.ID
	c.sleeping = true
	c.ready = true
	c.scroll = 7
	c.sel = "t1"
	c.open["t1"] = true
	c.sess.Apply(host.Sent{Text: "cached turn"}, time.Now())
	c.sess.Apply(headless.Result{Subtype: "success"}, time.Now())
	c.sess.Info.Sleeping = true
	oldSession := c.sess
	cmd := m.wakeHost(c, a, "one new message", nil, false)
	if cmd == nil || !c.waking {
		t.Fatal("explicit send did not begin wake")
	}
	c.input = []rune("next draft")
	if m.sendPane(c, false) != nil || string(c.input) != "next draft" {
		t.Fatal("second send during wake consumed or duplicated the draft")
	}
	if m.wakeHost(c, a, "one new message", nil, false) != nil {
		t.Fatal("duplicate wake scheduled")
	}
	result := cmd().(sheetMsg)
	replayCmd := result.apply(m)
	current := m.host
	if current == c || current.sess != oldSession || !current.ready || current.scroll != 7 || current.sel != "t1" || !current.open["t1"] || string(current.input) != "next draft" {
		t.Fatal("reconnect discarded visible state before replay")
	}
	defer current.client.Close()
	if current.sess.Info.Sleeping {
		t.Fatal("new connection retained old sleeping marker during replay")
	}
	replay := replayCmd().(replayMsg)
	if !replay.whole {
		t.Fatal("fake replay incomplete")
	}
	next := m.onReplay(replay)
	m.onHostLines(next().(hostLinesMsg))
	if sends.Load() != 1 {
		t.Fatalf("message sent %d times", sends.Load())
	}
	if len(current.sess.Turns) != 2 || current.sess.Turns[0].Prompt != "cached turn" || current.sess.Turns[1].Prompt != "one new message" {
		t.Fatalf("replay duplicated cached history: %+v", current.sess.Turns)
	}
	m.onHostLines(hostLinesMsg{owner: c, key: c.key, closed: true})
	if m.host != current || string(current.input) != "next draft" {
		t.Fatal("old socket close changed the new conversation")
	}
}

func TestSleepingWakeFailureRestoresUnsentDraft(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, c := draftModel()
	a := m.agentByKey(c.key)
	a.Rush = true
	a.ID = "missing-host"
	a.Kind = "claude"
	if err := os.MkdirAll(filepath.Dir(host.SockPath(a.ID)), 0700); err != nil {
		t.Fatal(err)
	}
	c.sleeping = true
	c.input = []rune("keep this input")
	c.back = 3
	c.rememberSleepDraft()
	c.input = nil
	cmd := m.wakeHost(c, a, "keep this input", nil, false)
	cmd().(sheetMsg).apply(m)
	if c.waking || !c.sleeping || string(c.input) != "keep this input" || c.back != 3 || len(c.sending) != 0 || !strings.Contains(m.status, "config") {
		t.Fatalf("waking=%v sleeping=%v draft=%q back=%d sending=%d error=%q", c.waking, c.sleeping, string(c.input), c.back, len(c.sending), m.status)
	}
}

func TestSleepingReadDoesNotDialOrStartHost(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, c := draftModel()
	a := m.agentByKey(c.key)
	a.Rush = true
	a.ID = "asleep"
	a.Kind = "claude"
	dir := filepath.Dir(host.SockPath(a.ID))
	_ = os.MkdirAll(dir, 0700)
	path := filepath.Join(dir, "history.jsonl")
	_ = os.WriteFile(path, []byte(`{"type":"user","message":{"role":"user","content":"kept"}}`+"\n"), 0600)
	a.TranscriptPath = path
	info := host.Info{ID: a.ID, Kind: "claude", Sleeping: true, State: "idle"}
	b, _ := jsonx.Marshal(info)
	_ = os.WriteFile(filepath.Join(dir, "info.json"), b, 0600)
	msg := openHost(a)().(hostOpenMsg)
	if msg.err != nil || msg.c == nil || !msg.c.sleeping || msg.c.client != nil || len(msg.c.sess.Turns) != 1 {
		t.Fatalf("sleeping snapshot did not open without a socket: %+v", msg)
	}
	if _, err := os.Stat(filepath.Join(dir, "wake.lock")); !os.IsNotExist(err) {
		t.Fatal("view-only read attempted Ensure")
	}
}

func TestSleepingSettingsWakeOnlyHost(t *testing.T) {
	for _, name := range []string{"model", "effort", "permissions"} {
		t.Run(name, func(t *testing.T) {
			sends, _, controls := sleepingSocket(t, "control")
			m, c := draftModel()
			m.w, m.h = 120, 40
			a := m.agentByKey(c.key)
			a.ID = "control"
			a.Kind = "claude"
			a.Rush = true
			c.id = a.ID
			c.sleeping = true
			c.ready = true
			c.input = []rune("do not send this draft")
			c.back = 3
			c.sess.Info.PermissionMode = "default"
			var cmd tea.Cmd
			if name == "permissions" {
				cmd = m.setPermission(c, "acceptEdits")
			} else {
				cmd = m.setArg(c, name, map[string]string{"model": "sonnet", "effort": "high"}[name])
			}
			if cmd == nil || !c.waking {
				t.Fatal("setting did not schedule a host-only wake")
			}
			result := cmd().(sheetMsg)
			batch := result.apply(m)
			if m.host == c || m.host.client == nil {
				t.Fatal("setting did not reconnect")
			}
			defer m.host.client.Close()
			for _, command := range batch().(tea.BatchMsg) {
				if command != nil {
					if msg, ok := command().(sheetMsg); ok {
						msg.apply(m)
					}
				}
			}
			select {
			case op := <-controls:
				want := name
				if want == "permissions" {
					want = "mode"
				}
				if op != want {
					t.Fatalf("got operation %s, want %s", op, want)
				}
			case <-time.After(time.Second):
				t.Fatal("setting was not sent to the host")
			}
			if sends.Load() != 0 || string(m.host.input) != "do not send this draft" || m.host.back != 3 || len(m.host.sending) != 0 {
				t.Fatal("changing a sleeping setting sent a prompt or lost its draft")
			}
		})
	}
}

func TestSleepingMarkerParksBeforeEOF(t *testing.T) {
	m, c := draftModel()
	c.input = []rune("saved")
	line := []byte(`{"type":"agtop_info","info":{"sleeping":true,"state":"idle"}}`)
	if cmd := m.onHostLines(hostLinesMsg{owner: c, key: c.key, lines: [][]byte{line}}); cmd != nil {
		t.Fatal("sleep marker scheduled another socket read")
	}
	if m.host != c || !c.sleeping || string(c.input) != "saved" {
		t.Fatal("sleep marker did not immediately preserve and park the pane")
	}
}

func TestSleepingEOFFallsBackToPersistedMarker(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, c := draftModel()
	c.id = "persisted"
	c.input = []rune("saved")
	dir := filepath.Dir(host.SockPath(c.id))
	_ = os.MkdirAll(dir, 0700)
	b, _ := jsonx.Marshal(host.Info{ID: c.id, State: "idle", Sleeping: true})
	_ = os.WriteFile(filepath.Join(dir, "info.json"), b, 0600)
	cmd := m.onHostLines(hostLinesMsg{owner: c, key: c.key, closed: true})
	if cmd == nil || m.host != c || !c.waking {
		t.Fatal("EOF discarded pane before checking persisted sleep marker")
	}
	cmd().(sheetMsg).apply(m)
	if m.host != c || !c.sleeping || c.waking || string(c.input) != "saved" {
		t.Fatal("persisted marker lost the sleeping pane")
	}
}

func TestSleepingColdOpenThroughSyncHostKeepsSetupWithoutHistory(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	m, _ := benchModel(120, 40)
	a := m.focused()
	a.Rush = true
	a.PID = 0
	a.ID = "cold-sleep"
	a.SessionID = ""
	a.TranscriptPath = ""
	a.History = ""
	m.dropHost()
	dir := filepath.Dir(host.SockPath(a.ID))
	_ = os.MkdirAll(dir, 0700)
	info := host.Info{ID: a.ID, Kind: "claude", Sleeping: true, State: "idle", PermissionMode: "plan", PermissionModes: []event.PermissionMode{{ID: "plan"}, {ID: "default"}}}
	b, _ := jsonx.Marshal(info)
	_ = os.WriteFile(filepath.Join(dir, "info.json"), b, 0600)
	cmd := m.syncHost()
	if cmd == nil {
		t.Fatal("empty-history sleeping Rush session was not opened")
	}
	msg := cmd().(hostOpenMsg)
	if msg.err != nil {
		t.Fatal(msg.err)
	}
	m.onHostOpen(msg)
	c := m.host
	if c == nil || !c.sleeping || c.client != nil || c.sess.Info.PermissionMode != "plan" || len(permissionChoices("claude", c)) != 2 {
		t.Fatal("cold open lost sleeping setup")
	}
	if m.syncHost() != nil {
		t.Fatal("cold view tried to wake host")
	}
	if _, err := os.Stat(filepath.Join(dir, "wake.lock")); !os.IsNotExist(err) {
		t.Fatal("cold open called Ensure")
	}
	whole := convo.New()
	m.takeWhole(c, whole)
	if c.sess.Info.PermissionMode != "plan" {
		t.Fatal("history expansion erased saved setup")
	}
}
