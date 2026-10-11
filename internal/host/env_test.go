package host

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Config.Env reaches Claude Code on its first start and on the start after
// a restart you ask for (a new effort, which only a new process takes).
func TestConfigEnvReachesClaude(t *testing.T) {
	bin := setup(t)
	envLog := filepath.Join(filepath.Dir(bin), "env.log")
	script := "#!/bin/sh\nprintf '%s\\n' \"$RUSH_TEST_MARK\" >> \"$(dirname \"$0\")/env.log\"\n" + strings.TrimPrefix(fakeClaude, "#!/bin/sh\n")
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	cfg, err := Spawn(Config{Owner: os.Getpid(), Cwd: filepath.Dir(bin), Prompt: "one", Binary: bin,
		Env: []string{"RUSH_TEST_MARK=card-42"}})
	if err != nil {
		t.Fatal(err)
	}
	c, err := Dial(cfg.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	defer c.Stop()
	allowAndIdle := func() {
		for {
			ev := next(t, c, func(ev any) bool {
				switch e := ev.(type) {
				case InfoEvent:
					return e.Info.State == "blocked" || e.Info.State == "idle" && e.Info.Detail != ""
				}
				return false
			}).(InfoEvent)
			if ev.Info.State == "idle" {
				return
			}
			_ = c.Allow("r1", nil, false)
		}
	}
	allowAndIdle()
	if err := c.SetEffort("high"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.ClaudePID == 0 })
	if err := c.Send("two"); err != nil {
		t.Fatal(err)
	}
	next(t, c, func(ev any) bool { i, ok := ev.(InfoEvent); return ok && i.Info.ClaudePID != 0 })
	var got []string
	for deadline := time.Now().Add(5 * time.Second); len(got) < 2 && time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		b, _ := os.ReadFile(envLog)
		got = strings.Fields(string(b))
	}
	if len(got) != 2 || got[0] != "card-42" || got[1] != "card-42" {
		t.Fatalf("claude saw RUSH_TEST_MARK %q over two starts", got)
	}
}
