package claude

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Session is a live Claude Code process as it registers itself under
// sessions/<pid>.json. Interactive terminals only appear here.
type Session struct {
	PID       int    `json:"pid"`
	SessionID string `json:"sessionId"`
	Cwd       string `json:"cwd"`
	Kind      string `json:"kind"` // bg or interactive
	Name      string `json:"name"`
	Status    string `json:"status"` // busy, idle, shell
	JobID     string `json:"jobId"`
	StartedMs int64  `json:"startedAt"`
	UpdatedMs int64  `json:"updatedAt"`
	StatusMs  int64  `json:"statusUpdatedAt"`
}

func (s Session) StatusAt() time.Time { return time.UnixMilli(s.StatusMs) }

func (s Session) StartedAt() time.Time { return time.UnixMilli(s.StartedMs) }
func (s Session) UpdatedAt() time.Time {
	if s.UpdatedMs == 0 {
		return s.StartedAt()
	}
	return time.UnixMilli(s.UpdatedMs)
}

// ReadSessions lists sessions whose process is still alive.
func ReadSessions(a Account) []Session {
	dir := filepath.Join(a.ConfigDir, "sessions")
	ents, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []Session
	for _, e := range ents {
		if !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		s, ok := ReadSession(filepath.Join(dir, e.Name()))
		if !ok || !Alive(s.PID) {
			continue
		}
		out = append(out, s)
	}
	return out
}

// ReadSession reads one session file, whether or not its process lives.
func ReadSession(path string) (Session, bool) {
	var s Session
	b, err := os.ReadFile(path)
	if err != nil || jsonx.Unmarshal(b, &s) != nil || s.PID <= 0 {
		return s, false
	}
	return s, true
}

// Alive reports whether a process exists.
func Alive(pid int) bool { return pid > 0 && syscall.Kill(pid, 0) == nil }
