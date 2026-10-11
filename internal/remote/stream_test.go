package remote

import (
	"bufio"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestSessionsStream(t *testing.T) {
	s := newServer(t)
	dir := filepath.Join(host.Root(), "c3c3c3c3")
	os.MkdirAll(dir, 0o700)
	write := func(name string) {
		b, _ := jsonx.Marshal(host.Info{ID: "c3c3c3c3", Name: name, State: "idle", UpdatedAt: time.Now()})
		if err := os.WriteFile(filepath.Join(dir, "info.json"), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("first")
	peer := httptest.NewServer(s.Handler())
	defer peer.Close()
	// the same serve reached as a hub's peer, through its proxy
	hub := &Server{Config: Config{Name: "hub", Peers: []Peer{{Name: "box", URL: peer.URL, Token: s.Token}}}, Token: strings.Repeat("b", 64)}
	hubSrv := httptest.NewServer(hub.Handler())
	defer hubSrv.Close()

	for _, c := range []struct{ name, url, tok string }{
		{"direct", peer.URL + "/api/sessions/stream", s.Token},
		{"through the hub", hubSrv.URL + "/m/box/api/sessions/stream", hub.Token},
	} {
		t.Run(c.name, func(t *testing.T) {
			write("first")
			req, _ := http.NewRequest("GET", c.url, nil)
			req.Header.Set("Authorization", "Bearer "+c.tok)
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()
			if ct := resp.Header.Get("Content-Type"); ct != "text/event-stream" {
				t.Fatalf("content type %q", ct)
			}
			lines := make(chan string, 16)
			go func() {
				sc := bufio.NewScanner(resp.Body)
				for sc.Scan() {
					if d, ok := strings.CutPrefix(sc.Text(), "data: "); ok {
						lines <- d
					}
				}
				close(lines)
			}()
			next := func() string {
				select {
				case l := <-lines:
					return l
				case <-time.After(5 * time.Second):
					t.Fatal("no event")
					return ""
				}
			}
			if l := next(); !strings.Contains(l, `"sessions":[`) || !strings.Contains(l, `"name":"first"`) {
				t.Fatalf("first event %s", l)
			}
			write("renamed")
			if l := next(); !strings.Contains(l, `"name":"renamed"`) {
				t.Fatalf("after a change %s", l)
			}
		})
	}
}
