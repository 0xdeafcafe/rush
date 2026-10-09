package remote

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// fakeServe is a serve that starts sessions in memory: it records each POST
// and lists what it started, so attach has something to show.
type fakeServe struct {
	mu    sync.Mutex
	posts []StartRequest
	fail  int // answer this status instead, when set
}

func (f *fakeServe) handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		f.mu.Lock()
		defer f.mu.Unlock()
		switch {
		case r.Method == http.MethodPost && r.URL.Path == "/api/sessions":
			var req StartRequest
			raw, _ := io.ReadAll(r.Body)
			_ = jsonx.Unmarshal(raw, &req)
			f.posts = append(f.posts, req)
			if f.fail != 0 {
				w.WriteHeader(f.fail)
				writeJSON(w, map[string]string{"Error": "that folder is rush's own"})
				return
			}
			writeJSON(w, f.session(len(f.posts)-1))
		case r.URL.Path == "/api/sessions":
			var l []Session
			for i := range f.posts {
				l = append(l, f.session(i))
			}
			writeJSON(w, l)
		default:
			http.NotFound(w, r)
		}
	})
}

func (f *fakeServe) session(i int) Session {
	p := f.posts[i]
	return Session{Info: host.Info{ID: "new0000" + string(rune('1'+i)), Name: p.Name, Cwd: p.Cwd, State: "idle", UpdatedAt: time.Now()}, Alive: true}
}

// The create is one POST with the folder as typed (the machine's to check),
// and the session it made shows here through attach as the machine's.
func TestStartOnPostsOnceAndAttachShowsIt(t *testing.T) {
	shortHome(t)
	f := &fakeServe{}
	srv := httptest.NewServer(f.handler())
	t.Cleanup(srv.Close)
	p := Peer{Name: "box", URL: srv.URL, Token: "t"}

	s, err := StartOn(context.Background(), p, StartRequest{Cwd: "~/proj", Prompt: "fix it", Name: "fix"})
	if err != nil {
		t.Fatal(err)
	}
	if len(f.posts) != 1 || f.posts[0].Cwd != "~/proj" || f.posts[0].Prompt != "fix it" {
		t.Fatalf("posts: %+v", f.posts)
	}
	runAttach(t, p)
	var got host.Info
	until(t, "the new session listed here", func() bool { i, ok := aliasOf(s.ID); got = i; return ok })
	if got.Remote != "box" || got.RemoteID != s.ID || got.ID == s.ID {
		t.Errorf("alias: %+v", got)
	}
	if strings.HasPrefix(got.Cwd, "~/proj") || got.Cwd == "~/proj" {
		t.Errorf("a remote folder shown as a local path: %q", got.Cwd)
	}
	if len(f.posts) != 1 {
		t.Errorf("attach posted again: %d", len(f.posts))
	}
}

// A refusal comes back as the machine's words, after one POST only; one with
// no answer says the session may exist.
func TestStartOnFailures(t *testing.T) {
	f := &fakeServe{fail: http.StatusForbidden}
	srv := httptest.NewServer(f.handler())
	p := Peer{Name: "box", URL: srv.URL, Token: "t"}
	_, err := StartOn(context.Background(), p, StartRequest{Cwd: "/x", Prompt: "p"})
	if err == nil || !strings.Contains(err.Error(), "rush's own") || len(f.posts) != 1 {
		t.Fatalf("err %v, posts %d", err, len(f.posts))
	}
	srv.Close()
	_, err = StartOn(context.Background(), p, StartRequest{Cwd: "/x", Prompt: "p"})
	if err == nil || !strings.Contains(err.Error(), "may or may not") {
		t.Fatalf("an unanswered start should say it may exist: %v", err)
	}
}

func TestAttached(t *testing.T) {
	shortHome(t)
	if Attached("box") {
		t.Fatal("attached with nothing running")
	}
	runAttach(t, peerOf(t))
	until(t, "attach holds its lock", func() bool { return Attached("box") })
}
