package ui

// clipLog is the start of a long text, enough to know it by in rush's log.
func clipLog(text string) string {
	r := []rune(text)
	if len(r) <= 300 {
		return text
	}
	return string(r[:300]) + "…"
}
