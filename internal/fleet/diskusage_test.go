package fleet

import (
	"fmt"
	"golang.org/x/sys/unix"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// However the platform walks a folder, it counts what statUsage does: every
// file's and folder's blocks, links not followed.
func TestDirUsageMatchesStat(t *testing.T) {
	dir := t.TempDir()
	for i, p := range []string{"a.txt", "b/c.bin", "b/d/e.txt", "b/d/f/g.txt", "h/i.txt", "long-" + strings.Repeat("n", 200)} {
		p = filepath.Join(dir, p)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, make([]byte, (i+1)*9000), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	for i := range 300 { // more than one call's worth of entries
		os.WriteFile(filepath.Join(dir, "h", "many-"+strings.Repeat("x", i%50)+string(rune('a'+i%26))+"-"+filepath.Base(t.Name())+string(rune('0'+i%10))+string(rune('A'+i/26))), []byte("x"), 0o644)
	}
	os.Symlink(filepath.Join(dir, "b"), filepath.Join(dir, "link"))
	os.Symlink("/", filepath.Join(dir, "root"))
	dirs := []TempDir{{Path: dir}, {Path: filepath.Join(dir, "missing")}, {Path: filepath.Join(dir, "link")}}
	if got, want := DiskUsageBackground(dirs), DiskUsage(dirs); got != want {
		t.Fatalf("background %d, foreground %d", got, want)
	}
	want, got := statUsage(dir), dirUsage(dir)
	if want == 0 || got != want {
		t.Fatalf("dirUsage %d, statUsage %d", got, want)
	}
}

func BenchmarkDirUsage(b *testing.B) {
	dir := os.Getenv("RUSH_DU_DIR")
	if dir == "" {
		b.Skip("RUSH_DU_DIR names a big folder to walk")
	}
	b.Run("bulk", func(b *testing.B) {
		for b.Loop() {
			dirUsage(dir)
		}
	})
	b.Run("stat", func(b *testing.B) {
		for b.Loop() {
			statUsage(dir)
		}
	})
}

// Run both paths against the same tree, excluding fixture creation from CPU
// and allocation measurements. CPU/wall shows the background duty cycle.
func BenchmarkDiskUsagePacing(b *testing.B) {
	dir := b.TempDir()
	for i := range 2000 {
		sub := filepath.Join(dir, fmt.Sprint(i))
		if err := os.Mkdir(sub, 0700); err != nil {
			b.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(sub, "file"), []byte("data"), 0600); err != nil {
			b.Fatal(err)
		}
	}
	dirs := []TempDir{{Path: dir}}
	for _, tc := range []struct {
		name string
		walk func([]TempDir) int64
	}{
		{"foreground", DiskUsage}, {"background", DiskUsageBackground},
	} {
		b.Run(tc.name, func(b *testing.B) {
			var before, after unix.Rusage
			unix.Getrusage(unix.RUSAGE_SELF, &before)
			start := time.Now()
			for b.Loop() {
				if tc.walk(dirs) == 0 {
					b.Fatal("empty size")
				}
			}
			elapsed := time.Since(start)
			unix.Getrusage(unix.RUSAGE_SELF, &after)
			cpu := after.Utime.Nano() + after.Stime.Nano() - before.Utime.Nano() - before.Stime.Nano()
			b.ReportMetric(float64(cpu)/float64(elapsed)*100, "cpu-percent")
		})
	}
}

func diskFixture(b testing.TB) string {
	dir := b.TempDir()
	for i := range 40 {
		d := filepath.Join(dir, "node_modules", fmt.Sprintf("pkg%d", i), "lib", "deep")
		if err := os.MkdirAll(d, 0o755); err != nil {
			b.Fatal(err)
		}
		for j := range 20 {
			os.WriteFile(filepath.Join(d, fmt.Sprintf("f%d.js", j)), make([]byte, 5000), 0o644)
		}
	}
	os.WriteFile(filepath.Join(dir, "main.go"), make([]byte, 5000), 0o644)
	return dir
}

func BenchmarkDiskUsageBackground(b *testing.B) {
	dir := diskFixture(b)
	dirs := []TempDir{{Path: dir}}
	b.ReportAllocs()
	for b.Loop() {
		DiskUsageBackground(dirs)
	}
}

func BenchmarkDiskUsageForeground(b *testing.B) {
	dir := diskFixture(b)
	dirs := []TempDir{{Path: dir}}
	b.ReportAllocs()
	for b.Loop() {
		DiskUsage(dirs)
	}
}

// A heavy folder's total is reused while it's unchanged and counted
// afresh once its own listing changes.
func TestHeavyFolderReused(t *testing.T) {
	dir := diskFixture(t)
	dirs := []TempDir{{Path: dir}}
	first := DiskUsageBackground(dirs)
	if first != DiskUsage(dirs) {
		t.Fatal("background and foreground differ")
	}
	nm := filepath.Join(dir, "node_modules")
	os.WriteFile(filepath.Join(nm, "new.js"), make([]byte, 40000), 0o644)
	if DiskUsageBackground(dirs) <= first {
		t.Fatal("a changed heavy folder was not counted afresh")
	}
}
