package convo

import (
	"bufio"
	"os"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// BenchmarkHistoryReal reads what a real transcript (RUSH_HISTORY) held an
// hour after it began, as a session resumed then would.
func BenchmarkHistoryReal(b *testing.B) {
	path := os.Getenv("RUSH_HISTORY")
	if path == "" {
		b.Skip("RUSH_HISTORY names a transcript")
	}
	f, err := os.Open(path)
	if err != nil {
		b.Skip(err)
	}
	var first time.Time
	sc := bufio.NewScanner(f)
	sc.Buffer(nil, 64<<20)
	for first.IsZero() && sc.Scan() {
		var l struct {
			Timestamp time.Time `json:"timestamp"`
		}
		_ = jsonx.Unmarshal(sc.Bytes(), &l)
		first = l.Timestamp
	}
	f.Close()
	b.ReportAllocs()
	for b.Loop() {
		History(path, first.Add(time.Hour))
	}
}
