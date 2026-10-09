package remote

import (
	"net/http"
	"slices"
	"sync"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
)

// Other is a session of an agent on this machine that rush didn't start:
// one running in a terminal, which can only be seen, or a past one, which
// can be resumed in rush.
type Other struct {
	Agent     string    `json:"agent"`
	Account   string    `json:"account,omitempty"`
	SessionID string    `json:"sessionId"`
	Name      string    `json:"name,omitempty"`
	Cwd       string    `json:"cwd,omitempty"`
	UpdatedAt time.Time `json:"updatedAt,omitzero"`
	Live      bool      `json:"live,omitzero"` // running outside rush now
}

const othersMost = 200

// others lists them, newest first, leaving out any a rush session is, and
// keeps the listing a minute: it reads the head of every transcript.
var others = struct {
	sync.Mutex
	at   time.Time
	list []Other
}{}

func listOthers() []Other {
	others.Lock()
	defer others.Unlock()
	if time.Since(others.at) < time.Minute {
		return others.list
	}
	ours := map[string]bool{}
	for _, i := range host.List() {
		ours[i.SessionID] = true
	}
	var out []Other
	seen := map[string]bool{}
	for _, a := range agent.All() {
		d, ok := a.(agent.Discoverer)
		if !ok {
			continue
		}
		for _, p := range a.Profiles() {
			add := func(ss []agent.Session, live bool) {
				for _, s := range ss {
					if s.ID == "" || s.Remote || ours[s.ID] || seen[s.ID] {
						continue
					}
					seen[s.ID] = true
					out = append(out, Other{Agent: string(a.Kind()), Account: p.Name, SessionID: s.ID, Name: s.Name,
						Cwd: s.Cwd, UpdatedAt: s.UpdatedAt, Live: live})
				}
			}
			add(d.Live(p), true)
			add(d.Past(p), false)
		}
	}
	slices.SortFunc(out, func(a, b Other) int { return b.UpdatedAt.Compare(a.UpdatedAt) })
	if len(out) > othersMost {
		out = out[:othersMost]
	}
	others.at, others.list = time.Now(), out
	return out
}

// pastOther is the listed past session with this id, if there is one.
func pastOther(sessionID string) (Other, bool) {
	for _, o := range listOthers() {
		if o.SessionID == sessionID && !o.Live {
			return o, true
		}
	}
	return Other{}, false
}

func (s *Server) others(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, listOthers())
}
