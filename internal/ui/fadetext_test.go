package ui

import (
	"fmt"
	"regexp"
	"testing"

	"github.com/0xdeafcafe/photon/theme"
)

// fadeText fades each text colour as the pattern it replaced did.
func TestFadeTextAsBefore(t *testing.T) {
	re := regexp.MustCompile(`38;2;(\d+);(\d+);(\d+)`)
	want := func(s string, by float64) string {
		return re.ReplaceAllStringFunc(s, func(code string) string {
			p := re.FindStringSubmatch(code)
			c := theme.Mix(theme.RGB{R: atoi8(p[1]), G: atoi8(p[2]), B: atoi8(p[3])}, painted.BG, by)
			return fmt.Sprintf("38;2;%d;%d;%d", c.R, c.G, c.B)
		})
	}
	for _, s := range []string{
		"", "plain", "\x1b[38;2;10;20;30mhi\x1b[0m", "\x1b[1;38;2;255;0;9m·\x1b[38;2;1;2;3mx",
		"38;2;", "38;2;1;2", "38;2;1;2;", "38;2;1;;3", "38;38;2;4;5;6m", "38;2;999;0;0m", "a38;2;7;8;9",
		"38;2;1;2;3;4;5", "x38;2;38;2;1;2;3m", "38;2;00012;7;99999999999999999999m",
	} {
		for _, by := range []float64{0.3, 0.75} {
			if got, w := fadeText(s, by), want(s, by); got != w {
				t.Fatalf("fadeText(%q, %v) = %q, want %q", s, by, got, w)
			}
		}
	}
}
