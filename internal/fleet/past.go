package fleet

import (
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// pastEvery is how often a past conversation's subagents are looked at
// again: none can start, so no oftener than its adapter lists it.
const pastEvery = 15 * time.Second

// pastKeys are past conversations' row keys, by session id, made once.
type pastKeys map[string]string

// branches are the conversations rush sessions left behind when rewound:
// each session shows them itself.
func (l *Loader) branches(hosted []host.Info, claimed map[string]bool) {
	for _, info := range hosted {
		cfg, ok := l.config(info)
		if !ok {
			continue
		}
		for _, b := range cfg.Branches {
			claimed[b.SessionID] = true
		}
	}
}

// config is the config session info was started with, read again only
// when it's changed.
func (l *Loader) config(info host.Info) (host.Config, bool) {
	path := filepath.Join(host.Root(), info.ID, "config.json")
	var v any
	if e, ok := l.files[path]; ok && (info.State == "stopped" || l.fresh(filepath.Dir(path), e.at)) {
		// Only its running host writes it, by renaming it in, which its
		// watched folder tells: hundreds aren't stat'd every reading.
		v = e.v
	} else {
		v = l.memo(path, func() any {
			cfg, err := host.ReadConfig(info.ID)
			if err != nil {
				return nil
			}
			return cfg
		})
	}
	cfg, ok := v.(host.Config)
	return cfg, ok
}

// pastAgents are an account's conversations nothing has open: a terminal
// session that has closed keeps its row, and ones from before rush ran
// are there too. A message resumes one in rush mode.
func (l *Loader) pastAgents(p agent.Profile, claimed, seen map[string]bool, now time.Time) []*Agent {
	ov := l.store.Overlay
	var out []*Agent
	d, ok := discoverer(p.Kind)
	if !ok {
		return nil
	}
	if l.pastKeys == nil {
		l.pastKeys = pastKeys{}
	}
	past := d.Past(p)
	for i := range past {
		s := &past[i]
		sid := s.ID
		if len(sid) < 8 || claimed[sid] {
			continue
		}
		key, ok := l.pastKeys[sid]
		if !ok {
			key = state.Key(p.Name, "i:"+sid[:8])
			l.pastKeys[sid] = key
		}
		f := pastFile{path: s.Transcript, mod: s.UpdatedAt, title: s.Name, cwd: s.Cwd, started: s.CreatedAt}
		if seen[key] {
			continue
		}
		seen[key] = true
		_, done := ov.Done[key]
		in := pastIn{profile: p, pastFile: f, name: ov.Names[key], done: done, group: ov.Groups[key],
			spend: l.spendVer[key], recent: now.Sub(f.mod) < 24*time.Hour}
		in.repo, in.branch = l.gitFor(firstNonEmpty(l.spend[key].Dir, f.cwd), now)
		if in.recent {
			in.subs = l.pastSubagents(p.Kind, key, f.path, now)
		}
		// Most rows are just as they were a second ago: the same row does.
		if r, ok := l.pastRows[key]; ok && r.in == in {
			out = append(out, r.a)
			continue
		}
		j := agent.Job{
			ID: sid[:8], Account: p.Name, Name: f.title, State: "stopped", Cwd: f.cwd,
			SessionID: sid, CreatedAt: f.started, UpdatedAt: f.mod, TranscriptPath: f.path,
		}
		a := &Agent{Job: j, Key: key, Acct: p, Kind: string(p.Kind), DisplayName: f.title, Past: true}
		if in.name != "" {
			a.DisplayName = in.name
		}
		a.Done, a.Group = done, in.group
		a.Repo, a.Branch = in.repo, in.branch
		a.Spend = l.spend[key]
		a.Subs = in.subs
		l.pastRows[key] = pastRow{in: in, a: a}
		out = append(out, a)
	}
	return out
}

// pastFile is one transcript as the adapter lists it.
type pastFile struct {
	path, title, cwd string
	mod, started     time.Time
}

// pastIn is everything a past conversation's row is made from.
type pastIn struct {
	profile agent.Profile
	pastFile
	name, group, repo, branch string
	done, recent              bool
	spend                     int // l.spendVer's
	subs                      agent.SubagentStats
}

type pastRow struct {
	in pastIn
	a  *Agent
}

// pastSubagents is subagents for a conversation nothing has open: none can
// start, so the folder is looked at again only as often as the listing,
// and its transcript isn't read for which are working: only one still
// writing is, its process gone or not.
func (l *Loader) pastSubagents(k agent.Kind, key, transcript string, now time.Time) agent.SubagentStats {
	if e, ok := l.subs[key]; ok && e.st.Direct+e.st.Nested == 0 && now.Sub(e.at) < pastEvery {
		return e.st
	}
	f, ok := agent.As[agent.RunFollower](k)
	if !ok {
		return agent.SubagentStats{}
	}
	st := f.CountSubagents(transcript, now)
	l.subs[key] = subsEntry{st: st, at: now}
	return st
}
