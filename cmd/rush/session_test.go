package main

import (
	"bytes"
	"encoding/base64"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// The test binary stands in for rush: host.Spawn runs `<exe> host run <id>`.
func TestMain(m *testing.M) {
	if len(os.Args) == 4 && os.Args[1] == "host" && os.Args[2] == "run" {
		if err := host.Run(os.Args[3]); err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
		os.Exit(0)
	}
	// Hosts the tests start write shims: into a cache of their own, not
	// the machine's, which sessions' shells run.
	d, _ := os.MkdirTemp("", "rush-cache")
	os.Setenv("RUSH_CACHE", d)
	code := m.Run()
	os.RemoveAll(d)
	os.Exit(code)
}

// fakeClaude logs its arguments and every message it is sent, and ends each
// turn at once.
const fakeClaude = `#!/bin/sh
printf '%s\n' "$*" >> "$(dirname "$0")/args.log"
sid=SID; prev=; for a in "$@"; do case "$prev" in --session-id|--resume) sid=$a ;; esac; prev=$a; done
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'","model":"claude-haiku-4-5","permissionMode":"default","tools":[]}'
while read -r line; do
  case "$line" in
  *'"type":"user"'*)
    printf '%s\n' "$line" >> "$(dirname "$0")/sent.log"
    echo '{"type":"assistant","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"ok"}]}}'
    echo '{"type":"result","subtype":"success","result":"ok","total_cost_usd":0.01}'
    ;;
  esac
done
`

func setup(t *testing.T) (bin string) {
	t.Helper()
	home, err := os.MkdirTemp("/tmp", "rush-cli-")
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("RUSH_HOME", home)
	t.Setenv("RUSH_SESSION", "") // not the session these tests may run in
	bin = filepath.Join(home, "claude")
	if err := os.WriteFile(bin, []byte(fakeClaude), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		for _, i := range host.List() {
			if c, err := host.Dial(i.ID); err == nil {
				_ = c.Stop()
				c.Close()
			}
		}
		// A stopped session's host stays up for the next message; the test
		// binary standing in for rush must not outlive the test. Only this
		// test's home is looked at.
		if os.Getenv("RUSH_HOME") != home {
			os.RemoveAll(home)
			return
		}
		for _, i := range host.List() {
			if i.HostPID > 0 {
				_ = syscall.Kill(i.HostPID, syscall.SIGTERM)
			}
		}
		for _, i := range host.List() {
			for deadline := time.Now().Add(3 * time.Second); i.HostPID > 0 && time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
				if syscall.Kill(i.HostPID, 0) != nil {
					break
				}
			}
			if i.HostPID > 0 {
				_ = syscall.Kill(i.HostPID, syscall.SIGKILL)
			}
		}
		os.RemoveAll(home)
	})
	return bin
}

func run(t *testing.T, stdin string, args ...string) (string, int) {
	t.Helper()
	var out, errOut bytes.Buffer
	code := sessionCmd(args, strings.NewReader(stdin), &out, &errOut)
	return out.String() + errOut.String(), code
}

func startJSON(t *testing.T, args ...string) map[string]any {
	t.Helper()
	out, code := run(t, "", append([]string{"start", "--json"}, args...)...)
	if code != 0 {
		t.Fatalf("start %v: exit %d: %s", args, code, out)
	}
	var v map[string]any
	if err := jsonx.Unmarshal([]byte(out), &v); err != nil {
		t.Fatalf("start printed %q: %v", out, err)
	}
	return v
}

func waitFor(t *testing.T, what string, ok func() bool) {
	t.Helper()
	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		if ok() {
			return
		}
	}
	t.Fatalf("timed out waiting for %s", what)
}

const sid = "11111111-2222-4333-8444-555555555555"

func TestStartIsIdempotentAndPrintsInfo(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	first := startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin, "--name", "card one",
		"--meta", "card=42", "--env", "KANBAN_CARD=42")
	for _, k := range []string{"id", "sessionId", "cwd", "name", "hostPid", "state", "startedAt", "updatedAt", "alive", "meta"} {
		if _, ok := first[k]; !ok {
			t.Errorf("start JSON has no %q: %v", k, first)
		}
	}
	if first["id"] != "11111111" || first["sessionId"] != sid || first["alive"] != true || first["name"] != "card one" {
		t.Fatalf("start: %v", first)
	}
	again := startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin)
	if again["hostPid"] != first["hostPid"] {
		t.Fatalf("a second start spawned another host: %v then %v", first["hostPid"], again["hostPid"])
	}
	cfg, err := host.ReadConfig("11111111")
	if err != nil {
		t.Fatal(err)
	}
	if len(cfg.Env) != 1 || cfg.Env[0] != "KANBAN_CARD=42" || cfg.Meta["card"] != "42" || cfg.Resume {
		t.Fatalf("config: %+v", cfg)
	}

	out, code := run(t, "", "info", "11111111", "--json")
	var info map[string]any
	if code != 0 || jsonx.Unmarshal([]byte(out), &info) != nil || info["alive"] != true || info["id"] != "11111111" {
		t.Fatalf("info: exit %d: %s", code, out)
	}
	out, code = run(t, "", "info", "deadbeef", "--json")
	if code != 1 || strings.TrimSpace(out) != `{"error":"not found"}` {
		t.Fatalf("info on no session: exit %d: %s", code, out)
	}
	if _, code := run(t, "", "start", "--resume", "--cwd", dir); code == 0 {
		t.Fatal("--resume without --session-id should fail")
	}
}

// One started from a rush session's shell is listed as that session's.
func TestStartFromASessionIsItsSubagent(t *testing.T) {
	bin := setup(t)
	t.Setenv("RUSH_SESSION", "parent00")
	startJSON(t, "--cwd", filepath.Dir(bin), "--session-id", sid, "--binary", bin)
	if cfg, err := host.ReadConfig("11111111"); err != nil || cfg.Meta["spawnedBy"] != "parent00" {
		t.Fatalf("config: %+v %v", cfg.Meta, err)
	}
}

func TestStopThenResume(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	first := startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin)
	if out, code := run(t, "", "stop", "11111111"); code != 0 {
		t.Fatalf("stop: %s", out)
	}
	out, _ := run(t, "", "info", "11111111", "--json")
	if !strings.Contains(out, `"alive":false`) || !strings.Contains(out, `"state":"stopped"`) {
		t.Fatalf("after stop: %s", out)
	}
	if _, code := run(t, "", "start", "--cwd", dir, "--session-id", sid, "--json"); code == 0 {
		t.Fatal("starting a stopped session without --resume should fail")
	}
	back := startJSON(t, "--cwd", dir, "--session-id", sid, "--resume")
	if back["alive"] != true || back["hostPid"] == first["hostPid"] {
		t.Fatalf("resume: %v", back)
	}
	cfg, _ := host.ReadConfig("11111111")
	if !cfg.Resume || cfg.Binary != bin {
		t.Fatalf("resumed config: %+v", cfg)
	}
}

func TestSendToStoppedHostResumesIt(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin, "--prompt-file", writeFile(t, dir, "p.txt", "first message\n"))
	sent := filepath.Join(dir, "sent.log")
	waitFor(t, "the first message", func() bool { b, _ := os.ReadFile(sent); return strings.Contains(string(b), "first message") })

	if out, code := run(t, "second message\n", "send", "11111111"); code != 0 || !strings.Contains(out, "sent to") {
		t.Fatalf("send: exit %d: %s", code, out)
	}
	waitFor(t, "the second message", func() bool { b, _ := os.ReadFile(sent); return strings.Contains(string(b), "second message") })

	run(t, "", "stop", "11111111")
	if out, code := run(t, "third message", "send", "11111111"); code != 0 || !strings.Contains(out, "resumed") {
		t.Fatalf("send to a stopped session: exit %d: %s", code, out)
	}
	waitFor(t, "the third message", func() bool { b, _ := os.ReadFile(sent); return strings.Contains(string(b), "third message") })
	b, _ := os.ReadFile(filepath.Join(dir, "args.log"))
	if !strings.Contains(string(b), "--resume "+sid) {
		t.Fatalf("claude was not resumed: %s", b)
	}
	if i, _ := host.ReadInfo("11111111"); i.State == "stopped" {
		t.Fatalf("host is not back: %+v", i)
	}
	if _, code := run(t, "", "send", "11111111"); code == 0 {
		t.Fatal("an empty message should fail")
	}
	if _, code := run(t, "hi", "send", "deadbeef"); code == 0 {
		t.Fatal("sending to no session should fail")
	}
	if out, code := run(t, "", "interrupt", "11111111"); code != 0 {
		t.Fatalf("interrupt: %s", out)
	}
}

func TestListFiltersByMeta(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--binary", bin, "--meta", "board=kanban", "--meta", "card=1")
	b := startJSON(t, "--cwd", dir, "--binary", bin, "--meta", "board=kanban", "--meta", "card=2")
	startJSON(t, "--cwd", dir, "--binary", bin)
	run(t, "", "stop", b["id"].(string))

	list := func(args ...string) []map[string]any {
		out, code := run(t, "", append([]string{"list", "--json"}, args...)...)
		var v []map[string]any
		if code != 0 || jsonx.Unmarshal([]byte(out), &v) != nil {
			t.Fatalf("list %v: exit %d: %s", args, code, out)
		}
		return v
	}
	if n := len(list()); n != 3 {
		t.Fatalf("list: %d sessions, want 3", n)
	}
	board := list("--meta", "board=kanban")
	if len(board) != 2 {
		t.Fatalf("board=kanban: %d sessions, want 2", len(board))
	}
	card := list("--meta", "board=kanban", "--meta", "card=2")
	if len(card) != 1 || card[0]["id"] != b["id"] || card[0]["alive"] != false || card[0]["state"] != "stopped" {
		t.Fatalf("card=2: %v", card)
	}
	if none := list("--meta", "board=other"); none == nil || len(none) != 0 {
		t.Fatalf("board=other: %v", none)
	}
}

func writeFile(t *testing.T, dir, name, body string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

// send --image puts each image after the text's [Image #N] for it; an image
// the text doesn't name goes with it as before.
func TestSendImagesAtTheirMarkers(t *testing.T) {
	bin := setup(t)
	dir := filepath.Dir(bin)
	startJSON(t, "--cwd", dir, "--session-id", sid, "--binary", bin)
	a := writeFile(t, dir, "a.png", "\x89PNG\r\n\x1a\nA")
	b := writeFile(t, dir, "b.png", "\x89PNG\r\n\x1a\nB")
	if out, code := run(t, "compare [Image #2] with [Image #1] please", "send", "11111111", "--image", a, "--image", b); code != 0 {
		t.Fatalf("send: exit %d: %s", code, out)
	}
	sent := filepath.Join(dir, "sent.log")
	var line string
	waitFor(t, "the message", func() bool {
		body, _ := os.ReadFile(sent)
		for _, l := range strings.Split(string(body), "\n") {
			if strings.Contains(l, "compare") {
				line = l
			}
		}
		return line != ""
	})
	var msg struct {
		Message struct {
			Content []struct {
				Type   string `json:"type"`
				Text   string `json:"text"`
				Source struct {
					Data string `json:"data"`
				} `json:"source"`
			} `json:"content"`
		} `json:"message"`
	}
	if err := jsonx.Unmarshal([]byte(line), &msg); err != nil {
		t.Fatalf("%v: %s", err, line)
	}
	var got []string
	for _, bl := range msg.Message.Content {
		if bl.Type == "text" {
			got = append(got, bl.Text)
			continue
		}
		raw, _ := base64.StdEncoding.DecodeString(bl.Source.Data)
		got = append(got, "img"+string(raw[len(raw)-1]))
	}
	if strings.Join(got, "|") != "compare [Image #2]|imgB| with [Image #1]|imgA| please" {
		t.Fatalf("content = %q", got)
	}
}
