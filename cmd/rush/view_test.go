package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Each example is a view rush view set takes as it is, and the guide
// shows every style.
func TestViewExamples(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	if err := os.MkdirAll(filepath.Dir(host.SockPath("s1")), 0o700); err != nil {
		t.Fatal(err)
	}
	for name, ex := range viewExamples {
		var v host.View
		if err := jsonx.Unmarshal([]byte(ex), &v); err != nil || v.Title == "" {
			t.Errorf("%s: %v", name, err)
			continue
		}
		v.Name = "ex"
		if err := host.SaveView("s1", v); err != nil {
			t.Errorf("%s: %v", name, err)
		}
	}
	for _, k := range host.BlockKinds {
		if !strings.Contains(viewGuide, `"kind":"`+k+`"`) {
			t.Errorf("the guide shows no %s", k)
		}
	}
}
