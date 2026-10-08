package claude

import (
	"encoding/json/jsontext"
	"os"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Worker is a live background session as the daemon tracks it.
type Worker struct {
	PID            int    `json:"pid"`
	SessionID      string `json:"sessionId"`
	RendezvousSock string `json:"rendezvousSock"`
	PtySock        string `json:"ptySock"`
	Cwd            string `json:"cwd"`
	Dispatch       struct {
		Short string `json:"short"`
	} `json:"dispatch"`
}

func (w Worker) Short() string {
	if w.Dispatch.Short != "" {
		return w.Dispatch.Short
	}
	if len(w.SessionID) >= 8 {
		return w.SessionID[:8]
	}
	return w.SessionID
}

type Roster struct {
	SupervisorPID int               `json:"supervisorPid"`
	Workers       map[string]Worker `json:"-"`
}

func ReadRoster(a Account) Roster {
	var raw struct {
		SupervisorPID int            `json:"supervisorPid"`
		Workers       jsontext.Value `json:"workers"`
	}
	r := Roster{Workers: map[string]Worker{}}
	b, err := os.ReadFile(a.RosterPath())
	if err != nil || jsonx.Unmarshal(b, &raw) != nil {
		return r
	}
	r.SupervisorPID = raw.SupervisorPID
	var list []Worker
	if jsonx.Unmarshal(raw.Workers, &list) != nil {
		var m map[string]Worker
		if jsonx.Unmarshal(raw.Workers, &m) == nil {
			for _, w := range m {
				list = append(list, w)
			}
		}
	}
	for _, w := range list {
		r.Workers[w.Short()] = w
	}
	return r
}

type PR = agent.PR

// ReadPRCache reuses Claude Code's own GitHub status cache; rush makes no GitHub calls.
func ReadPRCache(a Account) map[string]PR {
	m := map[string]PR{}
	b, err := os.ReadFile(a.PRCachePath())
	if err != nil {
		return m
	}
	_ = jsonx.Unmarshal(b, &m)
	return m
}

func PRNumberFromURL(u string) string {
	i := strings.LastIndex(u, "/pull/")
	if i < 0 {
		return ""
	}
	return u[i+6:]
}
