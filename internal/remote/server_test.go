package remote

import (
	"crypto/ecdsa"
	"crypto/sha256"
	"math/big"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

func newServer(t *testing.T, peers ...Peer) *Server {
	t.Setenv("RUSH_HOME", t.TempDir())
	return &Server{Config: Config{Name: "hub", Peers: peers}, Token: strings.Repeat("a", 64)}
}

func do(t *testing.T, h http.Handler, method, path, body string, hdr map[string]string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	r.Host = "rush.test"
	for k, v := range hdr {
		r.Header.Set(k, v)
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

func TestAuthOriginAndOps(t *testing.T) {
	s := newServer(t)
	h := s.Handler()
	bearer := map[string]string{"Authorization": "Bearer " + s.Token}
	cookie := func(extra map[string]string) map[string]string {
		m := map[string]string{"Cookie": cookieName + "=" + s.Token, "Content-Type": "application/json"}
		for k, v := range extra {
			m[k] = v
		}
		return m
	}
	for _, c := range []struct {
		name, method, path, body string
		hdr                      map[string]string
		want                     int
	}{
		{"shell is public", "GET", "/", "", nil, 200},
		{"api needs a token", "GET", "/api/sessions", "", nil, 401},
		{"wrong bearer", "GET", "/api/sessions", "", map[string]string{"Authorization": "Bearer nope"}, 401},
		{"bearer", "GET", "/api/sessions", "", bearer, 200},
		{"cookie", "GET", "/api/sessions", "", cookie(nil), 200},
		{"cookie post, same host", "POST", "/api/sessions/zz/op", `{"op":"stop"}`, cookie(map[string]string{"Origin": "http://rush.test"}), 404},
		{"cookie post, other origin", "POST", "/api/sessions/zz/op", `{"op":"stop"}`, cookie(map[string]string{"Origin": "https://evil.example"}), 403},
		{"cookie post, form", "POST", "/api/sessions/zz/op", `op=stop`, cookie(map[string]string{"Content-Type": "application/x-www-form-urlencoded"}), 403},
		{"cross-site fetch without origin", "POST", "/api/sessions/zz/op", `{"op":"stop"}`, cookie(map[string]string{"Sec-Fetch-Site": "cross-site"}), 403},
		{"login from another origin", "POST", "/api/login", `{"token":"` + s.Token + `"}`, map[string]string{"Origin": "https://evil.example"}, 401},
		{"login", "POST", "/api/login", `{"token":"` + s.Token + `"}`, map[string]string{"Origin": "http://rush.test"}, 200},
		{"no such session", "GET", "/api/sessions/nope/events", "", bearer, 404},
		{"path trick in id", "GET", "/api/sessions/..%2f..%2fetc/events", "", bearer, 404},
		{"no such machine", "GET", "/m/ghost/api/sessions", "", bearer, 404},
		{"machine needs token", "GET", "/m/ghost/api/sessions", "", nil, 401},
	} {
		if w := do(t, h, c.method, c.path, c.body, c.hdr); w.Code != c.want {
			t.Errorf("%s: %d, want %d: %s", c.name, w.Code, c.want, w.Body.String())
		}
	}
}

func TestHubProxiesWithPeerToken(t *testing.T) {
	peer := newServer(t)
	peer.Token = strings.Repeat("b", 64)
	var gotAuth, gotCookie string
	ph := peer.Handler()
	ps := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotAuth, gotCookie = r.Header.Get("Authorization"), r.Header.Get("Cookie")
		ph.ServeHTTP(w, r)
	}))
	defer ps.Close()
	hub := newServer(t, Peer{Name: "box", URL: ps.URL, Token: peer.Token})
	h := hub.Handler()
	w := do(t, h, "GET", "/m/box/api/sessions", "", map[string]string{"Cookie": cookieName + "=" + hub.Token})
	if w.Code != 200 {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
	if gotAuth != "Bearer "+peer.Token || gotCookie != "" {
		t.Errorf("peer saw auth %q cookie %q", gotAuth, gotCookie)
	}
	// only /api/ goes through, and only to a named peer
	if w := do(t, h, "GET", "/m/box/", "", map[string]string{"Authorization": "Bearer " + hub.Token}); w.Code != 404 {
		t.Errorf("non-api path: %d", w.Code)
	}
	// the hub's own token is no use at the peer, and the peer's none at the hub
	if w := do(t, h, "GET", "/api/sessions", "", map[string]string{"Authorization": "Bearer " + peer.Token}); w.Code != 401 {
		t.Errorf("peer token at hub: %d", w.Code)
	}
}

func TestRemoteOpsAllowlist(t *testing.T) {
	s := newServer(t)
	h := s.Handler()
	if err := os.MkdirAll(filepath.Join(host.Root(), "zz"), 0o700); err != nil {
		t.Fatal(err)
	}
	for _, op := range []string{"rewind", "restart", "ask", "relogin", "retitle", "compacted"} {
		w := do(t, h, "POST", "/api/sessions/zz/op", `{"op":"`+op+`"}`, map[string]string{"Authorization": "Bearer " + s.Token, "Content-Type": "application/json"})
		if w.Code != 403 {
			t.Errorf("%s: %d", op, w.Code)
		}
	}
}

func TestVapidSignature(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	p, err := LoadPush("https://rush.example")
	if err != nil {
		t.Fatal(err)
	}
	jwt, err := p.vapid("https://fcm.googleapis.com/fcm/send/abc")
	if err != nil {
		t.Fatal(err)
	}
	parts := strings.Split(jwt, ".")
	sig, _ := b64.DecodeString(parts[2])
	sum := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	if !ecdsa.Verify(&p.key.PublicKey, sum[:], new(big.Int).SetBytes(sig[:32]), new(big.Int).SetBytes(sig[32:])) {
		t.Fatal("signature doesn't verify")
	}
	claims, _ := b64.DecodeString(parts[1])
	if !strings.Contains(string(claims), `"aud":"https://fcm.googleapis.com"`) || !strings.Contains(string(claims), "https://rush.example") {
		t.Fatalf("%s", claims)
	}
	q, _ := LoadPush("")
	if q.PublicKey() != p.PublicKey() {
		t.Fatal("key not kept")
	}
}

func TestAllowedCwd(t *testing.T) {
	s := newServer(t)
	ws := t.TempDir()
	if err := s.allowedCwd(filepath.Join(state.Dir(), "logins")); err == nil {
		t.Error("rush's own folder allowed")
	}
	if err := s.allowedCwd(ws); err != nil {
		t.Errorf("no workspaces set: %v", err)
	}
	s.Config.Workspaces = []string{ws}
	sub := filepath.Join(ws, "p")
	_ = os.MkdirAll(sub, 0o700)
	if err := s.allowedCwd(sub); err != nil {
		t.Error(err)
	}
	if err := s.allowedCwd(filepath.Join(ws, "..")); err == nil {
		t.Error("parent of a workspace allowed")
	}
	out := t.TempDir()
	_ = os.Symlink(out, filepath.Join(ws, "link"))
	if err := s.allowedCwd(filepath.Join(ws, "link")); err == nil {
		t.Error("symlink out of a workspace allowed")
	}
}

// Every control the web app has, sent as the browser would send it, with
// a fake host in an isolated home standing in for the session: what
// reaches the host is what the app asked for, and nothing carries a path.
func TestWebControlsReachTheHost(t *testing.T) {
	shortHome(t)
	f := startFakeHost(t, host.Root(), "feedbeef", "job", "/work", approvalLine(t, "ap1"), questionLine(t, "q1"))
	var started StartRequest
	s := &Server{Config: Config{Name: "box"}, Token: strings.Repeat("a", 64), Start: func(r StartRequest) (host.Info, error) {
		started = r
		return f.info, nil
	}}
	h := s.Handler()
	hdr := map[string]string{"Cookie": cookieName + "=" + s.Token, "Content-Type": "application/json", "Origin": "http://rush.test"}
	post := func(path, body string) *httptest.ResponseRecorder { return do(t, h, "POST", path, body, hdr) }

	// create
	if w := post("/api/sessions", `{"cwd":"/work","agent":"claude","prompt":"go","name":"n"}`); w.Code != 200 {
		t.Fatalf("create: %d %s", w.Code, w.Body)
	}
	if started.Cwd != "/work" || started.Agent != "claude" || started.Prompt != "go" || started.Name != "n" {
		t.Fatalf("start request: %+v", started)
	}
	for _, c := range []struct {
		name, body string
		want       map[string]any
	}{
		{"send", `{"op":"send","text":"hello"}`, map[string]any{"op": "send", "text": "hello"}},
		{"steer", `{"op":"send","text":"turn left","guide":true}`, map[string]any{"op": "send", "text": "turn left", "guide": true}},
		{"stop the turn", `{"op":"interrupt"}`, map[string]any{"op": "interrupt"}},
		{"approve", `{"op":"allow","id":"ap1"}`, map[string]any{"op": "allow", "id": "ap1"}},
		{"approve always", `{"op":"allow","id":"ap1","always":true}`, map[string]any{"op": "allow", "id": "ap1", "always": true}},
		{"deny", `{"op":"deny","id":"ap1","message":"no"}`, map[string]any{"op": "deny", "id": "ap1", "message": "no"}},
		{"answer a question", `{"op":"allow","id":"q1","input":{"answers":{"Pick one":"B"},"questions":[]}}`, map[string]any{"op": "allow", "id": "q1"}},
		{"stop a task", `{"op":"stop_task","id":"t1"}`, map[string]any{"op": "stop_task", "id": "t1"}},
		{"queue send", `{"op":"queue_send","index":1,"was":"next"}`, map[string]any{"op": "queue_send", "index": 1.0, "was": "next"}},
		{"queue remove", `{"op":"queue_remove","index":0,"was":"x"}`, map[string]any{"op": "queue_remove", "was": "x"}},
		{"continue at the limit's reset", `{"op":"limit","now":true}`, map[string]any{"op": "limit", "now": true}},
		{"tell a subagent", `{"op":"tell","id":"sub1","text":"hurry"}`, map[string]any{"op": "tell", "id": "sub1", "text": "hurry"}},
		{"back", `{"op":"away"}`, map[string]any{"op": "away", "away": nil}},
	} {
		if w := post("/api/sessions/feedbeef/op", c.body); w.Code != http.StatusAccepted {
			t.Fatalf("%s: %d %s", c.name, w.Code, w.Body)
		}
		got := f.op(t)
		for k, v := range c.want {
			if got[k] != v {
				t.Errorf("%s: host got %v, want %s=%v", c.name, got, k, v)
			}
		}
	}
	// the answer's input arrives whole
	post("/api/sessions/feedbeef/op", `{"op":"allow","id":"q1","input":{"answers":{"Pick one":"B"}}}`)
	if in, _ := f.op(t)["input"].(map[string]any); in["answers"] == nil {
		t.Error("answer input lost")
	}
	// what a browser must not be able to do: attach files of this machine, or sneak another op
	post("/api/sessions/feedbeef/op", `{"op":"send","text":"x","images":["/etc/passwd"],"exchange":{"id":"z"}}`)
	if got := f.op(t); got["images"] != nil || got["exchange"] != nil {
		t.Errorf("a path or exchange reached the host: %v", got)
	}
	// away carries its times
	post("/api/sessions/feedbeef/op", `{"op":"away","away":{"from":"2026-10-09T10:00:00Z","until":"2026-10-09T12:00:00Z","next":"0001-01-01T00:00:00Z"}}`)
	if a, _ := f.op(t)["away"].(map[string]any); a["until"] != "2026-10-09T12:00:00Z" {
		t.Errorf("away arrived as %v", a)
	}
	if w := post("/api/sessions/feedbeef/op", `{"op":"away","away":{"every":1000}}`); w.Code != 400 {
		t.Errorf("a check-in every microsecond: %d", w.Code)
	}
	// an image arrives as bytes and reaches the host as a file in the session's scratch folder
	post("/api/sessions/feedbeef/op", `{"op":"send","text":"see","pictures":[{"type":"image/png","data":"aGk="}]}`)
	got := f.op(t)
	imgs, _ := got["images"].([]any)
	if len(imgs) != 1 {
		t.Fatalf("host got %v", got)
	}
	if p, _ := imgs[0].(string); !strings.HasPrefix(p, host.TempDir("feedbeef")) || !strings.HasSuffix(p, ".png") {
		t.Errorf("image at %q", p)
	} else if b, _ := os.ReadFile(p); string(b) != "hi" {
		t.Errorf("image holds %q", b)
	}
	for _, bad := range []string{
		`{"op":"send","text":"x","pictures":[{"type":"text/html","data":"aGk="}]}`,
		`{"op":"send","text":"x","pictures":[{"type":"image/png","data":""}]}`,
		`{"op":"allow","id":"ap1","pictures":[{"type":"image/png","data":"aGk="}]}`,
		`{"op":"send","pictures":[` + strings.Repeat(`{"type":"image/png","data":"aGk="},`, 8) + `{"type":"image/png","data":"aGk="}]}`,
	} {
		if w := post("/api/sessions/feedbeef/op", bad); w.Code != 400 {
			t.Errorf("%s: %d", bad, w.Code)
		}
	}
	if w := post("/api/sessions/feedbeef/op", `{"op":"rewind"}`); w.Code != 403 {
		t.Errorf("rewind: %d", w.Code)
	}
	f.noOp(t)
}
