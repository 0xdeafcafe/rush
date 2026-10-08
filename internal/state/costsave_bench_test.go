package state

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// BenchmarkCostCacheSave saves the cost cache you have, from a copy.
func BenchmarkCostCacheSave(b *testing.B) {
	home, _ := os.UserCacheDir()
	raw, err := os.ReadFile(filepath.Join(home, "rush", "costs.json"))
	if err != nil {
		b.Skip("no cost cache here")
	}
	c := &CostCache[map[string]any]{} // a transcript's totals, as JSON has them
	if err := jsonx.Unmarshal(raw, c); err != nil {
		b.Fatal(err)
	}
	b.Setenv("RUSH_CACHE", b.TempDir())
	b.SetBytes(int64(len(raw)))
	b.ReportAllocs()
	for b.Loop() {
		c.dirty = true
		if err := c.Save(); err != nil {
			b.Fatal(err)
		}
	}
}
