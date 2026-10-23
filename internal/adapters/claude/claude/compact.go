package claude

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/settingsfile"
)

// Claude Code's auto-compact window: CLAUDE_CODE_AUTO_COMPACT_WINDOW, else
// the settings' autoCompactWindow, else the model's own window. Compaction
// starts short of it by the room kept for a reply (the model's output
// limit, at most outputRoom) and a buffer of compactBuffer more.
const (
	minCompactWindow = 100_000
	maxCompactWindow = 1_000_000
	outputRoom       = 20_000
	compactBuffer    = 13_000
)

// CompactSource is what a session's auto-compact is read from: its
// process's environment, then each settings file it reads in order (a
// later one wins), and Claude Code's state file for whether it's on.
type CompactSource struct {
	Env      []string
	Settings []*settingsfile.File
	State    *settingsfile.File
}

// Compaction is where Claude Code compacts a session with src, as it
// works it out: none when auto-compact is off or no window is set apart
// from the model's.
func Compaction(src CompactSource) agent.Compaction {
	env := map[string]string{} // only the variables below are asked of it
	for _, e := range src.Env {
		if k, v, ok := strings.Cut(e, "="); ok && compactVar(k) {
			env[k] = v
		}
	}
	// A settings file's env block goes into the session's environment
	// over what it was started with.
	var setting int64
	for _, s := range src.Settings {
		if s == nil {
			continue
		}
		var block map[string]any
		if s.Get("env", &block) {
			for k, v := range block {
				if compactVar(k) {
					env[k] = envString(v)
				}
			}
		}
		var n float64
		if s.Get("autoCompactWindow", &n) && n == float64(int64(n)) && n >= minCompactWindow && n <= maxCompactWindow {
			setting = int64(n)
		}
	}
	if truthy(env["DISABLE_COMPACT"]) || truthy(env["DISABLE_AUTO_COMPACT"]) {
		return agent.Compaction{}
	}
	if src.State != nil {
		var on bool
		if src.State.Get("autoCompactEnabled", &on) && !on {
			return agent.Compaction{}
		}
	}
	room := int64(outputRoom)
	if n, ok := leadingInt(env["CLAUDE_CODE_MAX_OUTPUT_TOKENS"]); ok && n > 0 {
		room = min(room, n)
	}
	window := setting
	if v := env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"]; v != "" {
		if n, ok := leadingInt(v); ok && n > 0 {
			window = max(minCompactWindow, min(n, maxCompactWindow))
		}
	}
	if window == 0 {
		return agent.Compaction{}
	}
	return agent.Compaction{Window: window, Headroom: room + compactBuffer}
}

// compactVar is whether Compaction reads environment variable k.
func compactVar(k string) bool {
	switch k {
	case "DISABLE_COMPACT", "DISABLE_AUTO_COMPACT", "CLAUDE_CODE_MAX_OUTPUT_TOKENS", "CLAUDE_CODE_AUTO_COMPACT_WINDOW":
		return true
	}
	return false
}

// leadingInt is JavaScript's parseInt(v, 10): the digits v starts with,
// after spaces and a sign.
func leadingInt(v string) (int64, bool) {
	v = strings.TrimLeft(v, " \t\n\r")
	end := 0
	if end < len(v) && (v[end] == '-' || v[end] == '+') {
		end++
	}
	digits := end
	for end < len(v) && v[end] >= '0' && v[end] <= '9' {
		end++
	}
	if end == digits {
		return 0, false
	}
	n, err := strconv.ParseInt(v[:end], 10, 64)
	return n, err == nil
}

func truthy(v string) bool {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}

func envString(v any) string {
	switch v := v.(type) {
	case string:
		return v
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64)
	case nil:
		return ""
	}
	return fmt.Sprint(v)
}

// settingsMemo keeps settings files as last read, by path, read again
// only once they change: the list asks for every row on every reading.
var settingsMemo = struct {
	sync.Mutex
	files map[string]memoFile
}{files: map[string]memoFile{}}

type memoFile struct {
	mod  time.Time
	size int64
	file *settingsfile.File
}

// ReadSettingsCached is LoadSettingsFile, kept while the file doesn't
// change. Missing or unreadable is nil.
func ReadSettingsCached(path string) *settingsfile.File {
	fi, err := os.Stat(path)
	if err != nil {
		return nil
	}
	settingsMemo.Lock()
	defer settingsMemo.Unlock()
	if m, ok := settingsMemo.files[path]; ok && m.mod.Equal(fi.ModTime()) && m.size == fi.Size() {
		return m.file
	}
	f, err := settingsfile.Load(path)
	if err != nil {
		f = nil
	}
	settingsMemo.files[path] = memoFile{mod: fi.ModTime(), size: fi.Size(), file: f}
	return f
}
