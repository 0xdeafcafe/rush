package proc

import (
	"os"
	"testing"
)

// The process table read, as the machine view and fleet do it once a
// second. The Rust port's criterion bench (rust/crates/rush-sys/benches/
// proc.rs) measures the same three.

func BenchmarkSnapshot(b *testing.B) {
	b.ReportAllocs()
	for b.Loop() {
		Snapshot(nil)
	}
}

func BenchmarkSnapshotFillSelf(b *testing.B) {
	b.ReportAllocs()
	me := os.Getpid()
	prev := Snapshot(nil)
	for b.Loop() {
		t := Snapshot(prev)
		t.Fill(prev, []int{me})
	}
}

func BenchmarkFillEvery(b *testing.B) {
	b.ReportAllocs()
	prev := Snapshot(nil)
	roots := Snapshot(nil).Children[1]
	for b.Loop() {
		t := Snapshot(nil)
		t.Fill(prev, roots)
	}
}
