package convo

import (
	"strings"
)

// worktreeRe is a path into a worktree, as the usual layouts keep them:
// <repo>/.worktrees/<name>, .claude/worktrees/<name>, ~/worktrees/<name>.
// ponytail: by the path's shape alone, no git; ask git for a checkout kept
// anywhere else.
var worktreeRe = lazyRe(`/\.?worktrees/([^/\s"'\\]+)`)

// WorktreeIn is the worktree a command or a call's input points into,
// by name: the last one it names. "" when none.
func WorktreeIn(s string) string {
	all := worktreeRe().FindAllStringSubmatch(s, -1)
	if len(all) == 0 {
		return ""
	}
	return all[len(all)-1][1]
}

// DropCd is cmd without a leading cd into a worktree, which the row says
// by name instead: "cd …/.worktrees/x && make" is "make".
func DropCd(cmd string) string {
	if !strings.HasPrefix(cmd, "cd ") {
		return cmd
	}
	dir, rest, ok := strings.Cut(cmd[3:], " && ")
	if !ok || WorktreeIn(dir) == "" || strings.ContainsAny(dir, ";|&") {
		return cmd
	}
	return rest
}

// Worktree is the worktree the session's calls last reached into, by
// name: where a subagent works when it cds there itself. "" when none.
func (s *Session) Worktree() string { return s.worktree }

// MadeCall is whether the session made the call with this id: one it
// shows, or, followed light, one still out.
func (s *Session) MadeCall(id string) bool {
	if _, ok := s.byID[id]; ok {
		return true
	}
	_, ok := s.inFlight[id]
	return ok
}
