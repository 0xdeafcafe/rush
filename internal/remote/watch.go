package remote

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

func (s *Server) pushKey(w http.ResponseWriter, _ *http.Request) {
	if s.Push == nil {
		fail(w, http.StatusNotFound, "notifications aren't set up here")
		return
	}
	writeJSON(w, map[string]string{"key": s.Push.PublicKey()})
}

func (s *Server) pushSubscribe(w http.ResponseWriter, r *http.Request) {
	var sub Subscription
	if s.Push == nil {
		fail(w, http.StatusNotFound, "notifications aren't set up here")
		return
	}
	if err := readJSON(r, &sub); err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := s.Push.Add(sub); err != nil {
		fail(w, http.StatusBadRequest, err.Error())
		return
	}
	go s.Push.Send(context.WithoutCancel(r.Context()), Notice{Title: "rush", Body: "Notifications are on.", URL: "/", Tag: "rush-on"})
	w.WriteHeader(http.StatusNoContent)
}

// attention is what about a session is worth a notification.
func attention(i *host.Info) string {
	switch {
	case i.Needs != "":
		return "needs you"
	case i.Error != "" || i.Lost:
		return "stopped with an error"
	case i.State == "idle" && !i.IdleSince.IsZero():
		return "finished"
	}
	return ""
}

// Watch notifies, until ctx ends, when a session on this machine or a
// peer comes to need you, finishes, or fails. It looks every few
// seconds; what it saw first is taken as already known.
func (s *Server) Watch(ctx context.Context) {
	if s.Push == nil {
		return
	}
	seen := map[string]string{} // machine/id → attention, and when it went idle
	first := true
	tick := time.NewTicker(4 * time.Second) // ponytail: polls lists; a host-pushed feed if 4s is too slow
	defer tick.Stop()
	for {
		lists := map[string][]Session{s.Config.Name: s.sessions()}
		for _, p := range s.Config.Peers {
			if l, err := peerSessions(ctx, p); err == nil {
				lists[p.Name] = l
			}
		}
		for m, l := range lists {
			for i := range l {
				in := &l[i]
				a := attention(&in.Info)
				key := m + "/" + in.ID
				mark := a
				if a == "finished" {
					mark += in.IdleSince.String()
				}
				was, ok := seen[key]
				seen[key] = mark
				if first || !ok && a == "finished" || mark == was || a == "" {
					continue
				}
				s.Push.Send(ctx, Notice{Title: or(in.Name, in.ID), Body: m + " · " + a,
					URL: "/#/s/" + url.PathEscape(m) + "/" + in.ID, Tag: key})
			}
		}
		first = false
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
	}
}

func (s *Server) sessions() []Session {
	infos := host.List()
	out := make([]Session, 0, len(infos))
	for _, i := range infos {
		out = append(out, Session{Info: i, Alive: host.Alive(i.HostPID)})
	}
	return out
}

func peerSessions(ctx context.Context, p Peer) ([]Session, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	var out []Session
	return out, peerGet(ctx, p, "/api/sessions", &out)
}

// peerGet reads JSON from a peer's serve.
func peerGet(ctx context.Context, p Peer, path string, v any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, p.URL+path, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+p.Token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(io.LimitReader(resp.Body, 64<<20))
	if err != nil {
		return err
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("%s%s: %s", p.Name, path, resp.Status)
	}
	return jsonx.Unmarshal(b, v)
}

func or(a, b string) string {
	if a != "" {
		return a
	}
	return b
}
