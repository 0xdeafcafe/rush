package convo

import "strings"

// Markdown draws text with the conversation's light markdown (headings,
// lists, tables, fenced code), wrapped to w: a pull request's body, say.
func Markdown(s string, w int) []string {
	d := &drawer{s: New(), t: &Turn{}, o: Options{Width: w, Open: map[string]bool{}, NoRule: true}, cw: w}
	d.markdown(strings.TrimRight(strings.ReplaceAll(s, "\r\n", "\n"), " \t\n"), 1, cText, true)
	out := make([]string, 0, len(d.lines))
	for _, l := range squeezed(d.lines) {
		out = append(out, l.Text)
	}
	return out
}
