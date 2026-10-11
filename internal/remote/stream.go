package remote

import (
	"bytes"
	"net/http"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// sessionsStream is the session list as Server-Sent Events, so the web app
// needn't poll: {"sessions":[…]} at once and whenever the list changes,
// and a keepalive comment every 15s. A browser's stream starts with
// {"me":ID}, the device it's signed in as, for the list of devices.
func (s *Server) sessionsStream(w http.ResponseWriter, r *http.Request) {
	fl, _ := w.(http.Flusher)
	h := w.Header()
	h.Set("Content-Type", "text/event-stream")
	h.Set("Cache-Control", "no-store")
	h.Set("X-Accel-Buffering", "no")
	if c, err := r.Cookie(cookieName); err == nil {
		if d := s.devs.web(c.Value); d != nil {
			b, _ := jsonx.Marshal(map[string]string{"me": d.ID})
			_ = sse(w, fl, b)
		}
	}
	var l host.Lister // ponytail: reads the folder each second per stream; share one watcher if many watch
	var last []byte
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for n := 0; ; n++ {
		infos := l.List()
		out := make([]Session, 0, len(infos))
		for _, i := range infos {
			out = append(out, Session{Info: i, Alive: host.Alive(i.HostPID)})
		}
		b, err := jsonx.Marshal(map[string]any{"sessions": out})
		var werr error
		switch {
		case err == nil && !bytes.Equal(b, last):
			last, n = b, 0
			werr = sse(w, fl, b)
		case n%15 == 14:
			werr = sse(w, fl, nil)
		}
		if werr != nil {
			return
		}
		select {
		case <-r.Context().Done():
			return
		case <-tick.C:
		}
	}
}
