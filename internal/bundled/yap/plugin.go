// Package yap is the bundled plugin that shows a yap dictation in the
// message box it's going to: the words come up as yap hears them, the
// ones it's still guessing with a light passing over them, and a light
// runs round the box's edge. yap (github.com/0xdeafcafe/yap) is dictation
// for macOS on Apple's own speech model; it says what it hears on a socket
// of its own, which the window reads (Follow). The plugin is the switch.
package yap

import (
	"bufio"
	"context"
	"encoding/json"
	"encoding/json/jsontext"
	"io"
	"net"
	"os"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/rush/internal/plugin"
)

func init() { plugin.RegisterBundle(plugin.Bundle{Manifest: Manifest, Run: Run}) }

// Manifest is the yap plugin's.
var Manifest = plugin.Manifest{
	Name:     "yap",
	Category: "voice",
	Description: "Shows a yap dictation in the message box as you speak: the words as yap hears them, " +
		"a light over the ones it's still guessing, and a light round the box. Does nothing without yap.",
	Command:  []string{"rush"},
	MemoryMB: 16,
}

// Run is the plugin on its connection to the broker: it has nothing to do
// but be on.
func Run(rw io.ReadWriteCloser) error {
	conn := plugin.NewConn(rw, func(_ context.Context, method string, _ jsontext.Value) (any, error) {
		switch method {
		case "initialize":
			return map[string]any{}, nil
		case "tools.list":
			return map[string]any{"tools": []any{}}, nil
		}
		return nil, &plugin.Error{Code: plugin.CodeNoMethod, Message: "method not found: " + method}
	})
	<-conn.Done()
	return nil
}

// State is where a dictation is, as yap says.
type State struct {
	// Phase is hidden, listening, finishing (let go, tidying), done (the
	// text is in Settled and its paste follows) or failed.
	Phase    string `json:"phase"`
	Settled  string `json:"settled"`  // words it's sure of
	Guessing string `json:"guessing"` // words it may still change
	Level    int    `json:"level"`    // how loud, 0 to 100
	Locked   bool   `json:"locked"`   // hands free: it listens until fn is tapped
	App      string `json:"app"`      // the bundle id of the app it'll paste into
}

// Active is whether there's a dictation to show.
func (s State) Active() bool {
	return s.Phase == "listening" || s.Phase == "finishing" || s.Phase == "done"
}

// Socket is where yap says what it hears.
func Socket() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "yap", "live.sock")
}

// retry is how long Follow waits between looks for yap.
const retry = 2 * time.Second

// Follow hands out each state yap reports until stop closes, finding yap
// again whenever it starts. Losing yap mid-dictation ends the dictation.
func Follow(stop <-chan struct{}, out func(State)) {
	for {
		if c, err := net.Dial("unix", Socket()); err == nil {
			read(c, stop, out)
		}
		select {
		case <-stop:
			return
		case <-time.After(retry):
		}
	}
}

func read(c net.Conn, stop <-chan struct{}, out func(State)) {
	done := make(chan struct{})
	defer close(done)
	go func() {
		select {
		case <-stop:
		case <-done:
		}
		c.Close()
	}()
	sc := bufio.NewScanner(c)
	sc.Buffer(make([]byte, 0, 4096), 1<<20)
	shown := false
	for sc.Scan() {
		var s State
		if json.Unmarshal(sc.Bytes(), &s) != nil {
			continue
		}
		shown = s.Active()
		out(s)
	}
	if shown {
		out(State{Phase: "hidden"})
	}
}
