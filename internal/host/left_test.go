package host

import (
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

func TestShellWrites(t *testing.T) {
	got := shellWrites(`cd x && git clone https://g/r /tmp/r 2>/dev/null; curl -o ~/dl.zip u | tee "/var/log/a" > out.txt; cp a b /opt/c && mkdir -p $HOME/w rel`)
	want := []string{"/dev/null", "/tmp/r", "~/dl.zip", "/var/log/a", "/opt/c", "$HOME/w"}
	if !slices.Equal(got, want) {
		t.Fatalf("got %q\nwant %q", got, want)
	}
}

// The ledger keeps writes outside the project, its scratch, harness state
// and caches, from file tools and shell alike.
func TestNoteLeft(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	home, _ := os.UserHomeDir()
	proj := t.TempDir()
	s := &server{cfg: Config{ID: "s1", Cwd: proj}}
	call := func(k tool.Kind, in tool.Input) event.Part { return event.Part{Call: &tool.Call{Kind: k, Input: in}} }
	s.noteLeft(event.Message{Role: "assistant", Parts: []event.Part{
		call(tool.Write, tool.Input{Path: filepath.Join(proj, "in.go")}),
		call(tool.Write, tool.Input{Path: "/opt/elsewhere.txt"}),
		call(tool.Edit, tool.Input{Path: filepath.Join(home, ".claude", "settings.json")}),
		call(tool.Shell, tool.Input{Command: "echo hi > /tmp/log.txt 2>/dev/null; mkdir ~/.cache/x " + TempDir("s1") + "/y"}),
		call(tool.Read, tool.Input{Path: "/etc/hosts"}),
	}})
	var got []string
	for _, l := range s.info.Left {
		got = append(got, l.Path)
	}
	if !slices.Equal(got, []string{"/opt/elsewhere.txt", "/tmp/log.txt"}) || !s.info.Left[1].Shell {
		t.Fatalf("ledger: %+v", s.info.Left)
	}
}
