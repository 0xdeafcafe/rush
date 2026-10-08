package plugind

import (
	"context"
	"encoding/json/jsontext"
	"io"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// The test binary stands in for rush: a bundled plugin runs as
// `<exe> plugin run <name>`.
func TestMain(m *testing.M) {
	if len(os.Args) == 4 && os.Args[1] == "plugin" && os.Args[2] == "run" {
		registerE2E()
		if err := plugin.RunBundled(os.Args[3]); err != nil {
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func registerE2E() (forget func()) {
	return plugin.RegisterBundle(plugin.Bundle{
		Manifest: plugin.Manifest{Name: "bundle-e2e", Command: []string{"rush"}, UI: []string{plugin.UINotify},
			CLI: []plugin.CLISpec{{Name: "echo", Usage: "[words]", Description: "says them back"}}},
		Run: func(rw io.ReadWriteCloser) error {
			conn := plugin.NewConn(rw, func(_ context.Context, method string, params jsontext.Value) (any, error) {
				if method == "cli.run" {
					var r plugin.CLIRun
					_ = jsonx.Unmarshal(params, &r)
					return plugin.CLIResult{Stdout: strings.Join(r.Args, " ") + "\n", Stderr: r.Command, Exit: len(r.Args)}, nil
				}
				return map[string]any{}, nil
			})
			<-conn.Done()
			return nil
		},
	})
}

// A bundled plugin runs without an approval or a sandbox, as rush itself,
// and stops when it's turned off.
func TestBundledPluginRuns(t *testing.T) {
	defer registerE2E()()
	b := testBroker(t)
	if _, ok := plugin.Enabled()["bundle-e2e"]; !ok {
		t.Fatal("bundled plugin not enabled")
	}
	b.reload()
	r := b.runner("bundle-e2e")
	if r == nil {
		t.Fatal("the broker didn't take it on")
	}
	defer r.shutdown()
	for deadline := time.Now().Add(10 * time.Second); r.status().State != "running"; time.Sleep(20 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatalf("not running: %+v", r.status())
		}
	}
	// Its CLI command runs in it, and what it answers comes back.
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	res, err := b.cli(ctx, "bundle-e2e", plugin.CLIRun{Command: "echo", Args: []string{"hello", "there"}})
	if err != nil || res.Stdout != "hello there\n" || res.Stderr != "echo" || res.Exit != 2 {
		t.Fatalf("cli: %+v, %v", res, err)
	}
	if _, err := b.cli(ctx, "bundle-e2e", plugin.CLIRun{Command: "rm"}); err == nil {
		t.Fatal("a command it doesn't declare should be refused")
	}
	if err := plugin.SetBundled("bundle-e2e", false); err != nil {
		t.Fatal(err)
	}
	b.reload()
	if b.runner("bundle-e2e") != nil {
		t.Fatal("turned off, it should stop")
	}
}
