package claude

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"io"
	"os"
	"regexp"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
)

// heardTail is how much of a transcript's end Heard reads.
const heardTail = 4 << 20

// Heard is whether Claude Code took in a message of text sent at since:
// it writes each to its transcript as it queues it (with no content when
// it has images) and again as the turn's user message. known is false
// when the transcript or the text can't say.
func (c *conn) Heard(text string, since time.Time) (heard, known bool) {
	sid := c.sid.Load()
	if sid == nil || *sid == "" {
		return false, false
	}
	return heardIn(c.acct.FindTranscript(c.dir, *sid), text, since)
}

func heardIn(path, text string, since time.Time) (heard, known bool) {
	snip, ok := heardSnippet(text)
	if !ok {
		return false, false
	}
	f, err := os.Open(path)
	if err != nil {
		return false, false
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.Size() > heardTail {
		_, _ = f.Seek(-heardTail, io.SeekEnd)
	}
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64<<10), 32<<20)
	from := since.Add(-2 * time.Second) // its clock and ours, a write apart
	for sc.Scan() {
		l := sc.Bytes()
		var line struct {
			Type      string         `json:"type"`
			Operation string         `json:"operation"`
			Timestamp time.Time      `json:"timestamp"`
			Content   jsontext.Value `json:"content"`
		}
		if jsonx.Unmarshal(l, &line) != nil || line.Timestamp.Before(from) {
			continue
		}
		if bytes.Contains(l, snip) || line.Type == "queue-operation" && line.Operation == "enqueue" && len(line.Content) == 0 {
			return true, true
		}
	}
	return false, sc.Err() == nil
}

// marker is an image's place in a message's text: [Image #2].
var marker = regexp.MustCompile(`\[Image #\d+\]`)

// heardSnippet is the end of text's last line as the transcript writes it,
// JSON-escaped: the part of a message least like anything else.
func heardSnippet(text string) ([]byte, bool) {
	text = strings.TrimSpace(marker.ReplaceAllString(text, ""))
	if i := strings.LastIndexByte(text, '\n'); i >= 0 {
		text = strings.TrimSpace(text[i+1:])
	}
	for utf8.RuneCountInString(text) > 32 {
		_, n := utf8.DecodeRuneInString(text)
		text = text[n:]
	}
	if text == "" || strings.ContainsFunc(text, func(r rune) bool { return r < 0x20 || r == 0x2028 || r == 0x2029 }) {
		return nil, false
	}
	return []byte(strings.NewReplacer(`\`, `\\`, `"`, `\"`).Replace(text)), true
}
