package convo

import (
	"maps"
	"strings"
	"sync/atomic"

	"github.com/0xdeafcafe/rush/internal/community"
)

// Handle is an agent you can tag: its @name, the colour it goes by in the
// list, and its title.
type Handle struct{ Name, Tint, Title string }

// handles are the agents a tag names, by tag in lower case: any case
// tags them.
var (
	handles    atomic.Pointer[map[string]Handle]
	handlesGen atomic.Int64
)

// SetHandles is who each tag names now, by tag in lower case. What was
// drawn with the ones before is drawn again.
func SetHandles(hs map[string]Handle) {
	if old := handles.Load(); old != nil && maps.Equal(*old, hs) {
		return
	}
	handles.Store(&hs)
	handlesGen.Add(1)
	lookupsGen.Add(1)
}

// HandleOf is the agent tag names, any case, with or without its @.
func HandleOf(tag string) (Handle, bool) {
	hs := handles.Load()
	if hs == nil {
		return Handle{}, false
	}
	h, ok := (*hs)[strings.ToLower(strings.TrimPrefix(tag, "@"))]
	return h, ok
}

// AgentLink is where a tag of h links: a click on it opens that agent.
func AgentLink(h Handle) string { return "rush:agent/" + h.Name }

// agentTag draws tag, as written, in its agent's colour and linked to it.
func agentTag(h Handle, tag, base string) string {
	return reset + "\x1b]8;;" + AgentLink(h) + "\x1b\\" + paint(h.Tint+bold, tag) + "\x1b]8;;\x1b\\" + base
}

var sgrEnd = lazyRe(`\x1b\[[0-9;]*m$`)

// agentLinks draws each @tag in s that names an agent as agentTag does. A
// tag starts a word: the @ in an email or a path is no tag.
func agentLinks(s, base string) string {
	if !strings.Contains(s, "@") || handles.Load() == nil {
		return s
	}
	var b strings.Builder
	last := 0
	for _, at := range community.MentionRE().FindAllStringIndex(s, -1) {
		i, j := at[0], at[1]
		if i > 0 && !strings.ContainsRune(" \t([{\"'`*_", rune(s[i-1])) && !sgrEnd().MatchString(s[:i]) {
			continue
		}
		if j < len(s) && (s[j] == '/' || s[j] == '.' && j+1 < len(s) && s[j+1] != ' ') {
			continue // a path or a file: @src/x.go
		}
		if strings.Count(s[:i], "\x1b]8;;")%2 == 1 {
			continue // already a link's words
		}
		h, ok := HandleOf(s[i:j])
		if !ok {
			continue
		}
		b.WriteString(s[last:i])
		b.WriteString(agentTag(h, s[i:j], base))
		last = j
	}
	if last == 0 {
		return s
	}
	b.WriteString(s[last:])
	return b.String()
}
