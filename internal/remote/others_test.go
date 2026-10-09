package remote

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
)

// outsideAgent has a session running in a terminal, two past ones, and one
// that is a rush session's.
type outsideAgent struct{}

func init() { agent.Register(outsideAgent{}) }

func (outsideAgent) Kind() agent.Kind                          { return "outside" }
func (outsideAgent) Name() string                              { return "outside" }
func (outsideAgent) Features() map[agent.Feature]agent.Support { return nil }
func (outsideAgent) Level() agent.Level                        { return 0 }
func (outsideAgent) Profiles() []agent.Profile {
	return []agent.Profile{{Kind: "outside", Name: "work"}}
}
func (outsideAgent) Live(agent.Profile) []agent.Session {
	return []agent.Session{{ID: "live-1", Name: "in a terminal", Cwd: "/w"}}
}
func (outsideAgent) Past(agent.Profile) []agent.Session {
	t := time.Now()
	return []agent.Session{{ID: "past-old", Cwd: "/w", UpdatedAt: t.Add(-time.Hour)},
		{ID: "past-new", Name: "the bug", Cwd: "/w", UpdatedAt: t}, {ID: "ours-1", Cwd: "/w"}}
}

func TestOthersListAndResume(t *testing.T) {
	var started StartRequest
	s := newServer(t)
	s.Start = func(r StartRequest) (host.Info, error) { started = r; return host.Info{ID: "abcd1234"}, nil }
	others.at = time.Time{}
	dir := filepath.Join(host.Root(), "0000aaaa")
	os.MkdirAll(dir, 0o700)
	b, _ := jsonx.Marshal(host.Info{ID: "0000aaaa", SessionID: "ours-1", Cwd: "/w"})
	os.WriteFile(filepath.Join(dir, "info.json"), b, 0o600)
	h := s.Handler()
	bearer := map[string]string{"Authorization": "Bearer " + s.Token, "Content-Type": "application/json"}

	w := do(t, h, "GET", "/api/others", "", bearer)
	var list []Other
	if err := jsonx.Unmarshal(w.Body.Bytes(), &list); err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, o := range list {
		if o.Agent == "outside" {
			got = append(got, o.SessionID+map[bool]string{true: "*"}[o.Live])
		}
	}
	if strings.Join(got, " ") != "past-new past-old live-1*" {
		t.Errorf("listed %v (newest first, the rush one left out)", got)
	}

	if w := do(t, h, "POST", "/api/sessions", `{"resume":"past-new","cwd":"/elsewhere","agent":"claude"}`, bearer); w.Code != 200 {
		t.Fatalf("resume: %d %s", w.Code, w.Body)
	}
	if started.Resume != "past-new" || started.Agent != "outside" || started.Cwd != "/w" {
		t.Errorf("started %+v (its own agent and folder)", started)
	}
	for _, id := range []string{"live-1", "nope", "ours-1"} {
		if w := do(t, h, "POST", "/api/sessions", `{"resume":"`+id+`"}`, bearer); w.Code != 404 {
			t.Errorf("resume %s: %d", id, w.Code)
		}
	}
}
