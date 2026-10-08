package convo

import (
	"slices"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// A command that sends its output to a file says where to find it.
func TestJobWrites(t *testing.T) {
	s := New()
	s.Info = host.Info{Cwd: "/repo"}
	cmd := `cd /lw/langwatch; go run ./cmd/visualdiff run > .visualdiff.log 2>&1 & echo hi >/dev/null; make 2>&1 | tee -a "build.log"; echo x >> $OUT; cd sub && ls > ~/ls.txt`
	in, _ := jsonx.Marshal(map[string]string{"command": cmd})
	s.byID = map[string]*Step{"b1": {ID: "b1", Tool: "Bash", Input: in}}
	got := s.JobWrites(&Job{ToolUseID: "b1"})
	if !slices.Contains(got, "/lw/langwatch/.visualdiff.log") || !slices.Contains(got, "/lw/langwatch/build.log") || len(got) != 3 {
		t.Fatalf("JobWrites = %q", got)
	}
}
