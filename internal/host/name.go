package host

import (
	"path/filepath"
	"regexp"
	"strings"
)

var (
	pasteRe    = regexp.MustCompile(`(?s)<pasted_content[^>]*>.*?</pasted_content[^>]*>`)
	pasteTagRe = regexp.MustCompile(`</?pasted_content[^>]*>`)
	imageRe    = regexp.MustCompile(`\[Image #\d+\]`)
)

// NameFrom is a session's name from its first message, until it names
// itself: the first few words, with what was pasted and images left out,
// unless what was pasted is all there is.
func NameFrom(text string) string {
	words := strings.Fields(imageRe.ReplaceAllString(pasteRe.ReplaceAllString(text, " "), " "))
	if len(words) == 0 {
		words = strings.Fields(imageRe.ReplaceAllString(pasteTagRe.ReplaceAllString(text, " "), " "))
	}
	if len(words) > 6 {
		words = words[:6]
	}
	n := strings.Join(words, " ")
	if r := []rune(n); len(r) > 48 {
		n = string(r[:47]) + "…"
	}
	return n
}

// FreshName is what a session started without a message is called until
// its first one names it.
func FreshName(dir string) string { return "fresh session in " + filepath.Base(dir) }
