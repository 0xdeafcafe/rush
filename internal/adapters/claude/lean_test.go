package claude

import (
	"bytes"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

func TestLeanEnv(t *testing.T) {
	cache := t.TempDir()
	t.Setenv("RUSH_CACHE", cache)
	t.Setenv("BUN_OPTIONS", "")
	t.Setenv(dfgOff, "")
	p := filepath.Join(cache, "claude", "lean-preload.js")
	if got, want := leanEnv(), []string{"BUN_OPTIONS=--preload " + p, dfgOff + "=0"}; !slices.Equal(got, want) {
		t.Fatalf("leanEnv() = %q, want %q", got, want)
	}
	if b, err := os.ReadFile(p); err != nil || !bytes.Equal(b, leanPreload) {
		t.Fatalf("preload on disk = %q, %v", b, err)
	}
	for _, own := range []string{"BUN_OPTIONS", dfgOff} {
		t.Run("your own "+own, func(t *testing.T) {
			t.Setenv(own, "x")
			if got := leanEnv(); got != nil {
				t.Fatalf("leanEnv() = %q with %s set, want nothing", got, own)
			}
		})
	}
}
