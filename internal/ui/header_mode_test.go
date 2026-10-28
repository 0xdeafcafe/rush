package ui

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
)

// The mode's place in the header is where it stands as a word of its own,
// not inside another one.
func TestSegAt(t *testing.T) {
	row := "  autofix  ● working   opus 5 · auto · 3 files "
	right := "opus 5 · auto · 3 files"
	if at, ok := segAt(row, right, "auto", true); !ok || at != cellw.String(row[:strings.LastIndex(row, "auto")]) {
		t.Errorf("segAt = %d %v", at, ok)
	}
	if _, ok := segAt(row, right, "plan", true); ok {
		t.Error("plan isn't there")
	}
}
