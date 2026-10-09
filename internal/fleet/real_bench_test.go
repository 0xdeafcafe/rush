package fleet

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/state"
)

// On this machine's real agents: RUSH_REAL=1 go test -bench Real ./internal/fleet
func BenchmarkLoadReal(b *testing.B) {
	if os.Getenv("RUSH_REAL") == "" {
		b.Skip("set RUSH_REAL=1")
	}
	l := NewLoader(state.Load())
	l.Load(true)
	b.ReportAllocs()
	for b.Loop() {
		l.Load(true)
	}
}

func BenchmarkScanReal(b *testing.B) {
	if os.Getenv("RUSH_REAL") == "" {
		b.Skip("set RUSH_REAL=1")
	}
	l := NewLoader(state.Load())
	snap := l.Load(false)
	var targets []Target
	for _, a := range snap.Agents {
		if a.TranscriptPath != "" {
			targets = append(targets, Target{Key: a.Key, Path: a.TranscriptPath})
		}
	}
	sc := NewScanner()
	sc.Run(targets)
	b.ReportAllocs()
	for b.Loop() {
		sc.Run(targets)
	}
}

// A load after one watched file changed, as the UI's tick makes one while
// agents write, against a sweep's, which trusts no watch (what every load
// cost before): RUSH_REAL=1 go test -bench OneChange ./internal/fleet
func BenchmarkLoadRealOneChange(b *testing.B) {
	if os.Getenv("RUSH_REAL") == "" {
		b.Skip("set RUSH_REAL=1")
	}
	touch := filepath.Join(state.Dir(), ".bench-touch")
	defer os.Remove(touch)
	for _, sweep := range []bool{false, true} {
		name := "one-change"
		if sweep {
			name = "sweep"
		}
		b.Run(name, func(b *testing.B) {
			l := NewLoader(state.Load())
			l.Watch(time.Hour)
			for range 3 { // watching, then told what's newly watched, then trusted
				l.Load(true)
			}
			b.ReportAllocs()
			for b.Loop() {
				os.Remove(touch) // a new entry: its folder is told
				_ = os.WriteFile(touch, []byte(time.Now().String()), 0o644)
				if sweep {
					l.watching.swept = time.Time{}
				}
				l.Settle()
				l.Load(true)
			}
		})
	}
}
