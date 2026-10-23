package fleet

import (
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
	"time"
)

// Finished hosts are often hundreds of rows. Their transcript locations, git
// metadata and subagent directories do not need restatting on each live event.
// Host changes invalidate immediately; external file/git changes refresh in 15s.
// Overlay names/groups and spend are applied outside this cache on every load.
type idleHostedRow struct {
	profile         agent.Profile
	updated, at     time.Time
	sid, cwd, state string
	sleeping        bool
	row             Agent
}

func (l *Loader) hostedBase(p agent.Profile, info host.Info, tab *proc.Table, now time.Time) *Agent {
	idle := info.State == "stopped" || info.Sleeping
	if idle && !l.quick {
		if e, ok := l.idleRows[info.ID]; ok && e.profile == p && e.updated.Equal(info.UpdatedAt) && e.sid == info.SessionID && e.cwd == info.Cwd && e.state == info.State && e.sleeping == info.Sleeping && now.Sub(e.at) < pastEvery {
			a := e.row
			return &a
		}
	}
	watched := len(l.subWatch)
	a := l.hosted(p, info, tab, now)
	if a.TranscriptPath != "" {
		gone := a.PID == 0 || info.Proto >= 3 && info.ClaudePID == 0 && info.State != "working"
		a.Subs, a.Subagents = l.subagents(a.Acct.Kind, a.Key, a.TranscriptPath, gone, now)
	}
	if idle {
		// Read again on its own clock, an idle row's files aren't watched:
		// watched only by the load that read them, they were opened and
		// closed again every time, hundreds at once.
		l.subWatch = l.subWatch[:watched]
	}
	if idle && !l.quick {
		if l.idleRows == nil {
			l.idleRows = map[string]idleHostedRow{}
		}
		l.idleRows[info.ID] = idleHostedRow{profile: p, updated: info.UpdatedAt, at: now, sid: info.SessionID, cwd: info.Cwd, state: info.State, sleeping: info.Sleeping, row: *a}
	} else {
		delete(l.idleRows, info.ID)
	}
	return a
}
