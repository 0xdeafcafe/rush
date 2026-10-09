package remote

import (
	"net/http"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Each sign-in is a device of its own: its cookie is its own, it lists with
// a name and when it was last seen, and revoking it locks only it out. A
// cookie from before devices becomes one at the app's first request.
func TestDevicesSignInAndRevoke(t *testing.T) {
	s := newServer(t)
	h := s.Handler()
	bearer := map[string]string{"Authorization": "Bearer " + s.Token, "Content-Type": "application/json"}
	signIn := func(ua string) string {
		w := do(t, h, "POST", "/api/login", `{"token":"`+s.Token+`"}`, map[string]string{"Origin": "http://rush.test", "Content-Type": "application/json", "User-Agent": ua})
		c := w.Result().Cookies()
		if w.Code != 200 || len(c) != 1 || c[0].Value == s.Token {
			t.Fatalf("sign in: %d %v", w.Code, c)
		}
		return c[0].Value
	}
	phone := signIn("Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) Version/19.0 Mobile/15E148 Safari/604.1")
	laptop := signIn("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/140.0 Safari/537.36")
	as := func(tok string) map[string]string { return map[string]string{"Cookie": cookieName + "=" + tok} }
	if w := do(t, h, "GET", "/api/sessions", "", as(phone)); w.Code != 200 {
		t.Fatalf("the phone's cookie: %d", w.Code)
	}
	var list []Device
	w := do(t, h, "GET", "/api/devices", "", bearer)
	if err := jsonx.Unmarshal(w.Body.Bytes(), &list); err != nil || len(list) != 2 || strings.Contains(w.Body.String(), "tokenHash") {
		t.Fatalf("devices: %s", w.Body)
	}
	names := list[0].Name + "," + list[1].Name
	if !strings.Contains(names, "iPhone · Safari") || !strings.Contains(names, "Mac · Chrome") {
		t.Errorf("names: %s", names)
	}
	var id string
	for _, d := range list {
		if strings.HasPrefix(d.Name, "iPhone") {
			id = d.ID
		}
	}
	if w := do(t, h, "POST", "/api/devices/"+id+"/revoke", "{}", bearer); w.Code != 200 {
		t.Fatalf("revoke: %d %s", w.Code, w.Body)
	}
	if w := do(t, h, "GET", "/api/sessions", "", as(phone)); w.Code != http.StatusUnauthorized {
		t.Errorf("a revoked phone: %d", w.Code)
	}
	if w := do(t, h, "GET", "/api/sessions", "", as(laptop)); w.Code != 200 {
		t.Errorf("the laptop after the phone's revoked: %d", w.Code)
	}

	w = do(t, h, "GET", "/api/machines", "", as(s.Token))
	if c := w.Result().Cookies(); w.Code != 200 || len(c) != 1 || c[0].Value == s.Token {
		t.Fatalf("an old cookie isn't made a device: %d %v", w.Code, c)
	}
	if len(s.devs.all()) != 2 {
		t.Errorf("%d devices after the old cookie", len(s.devs.all()))
	}
}

// Revoking a device ends what it has open, such as an event stream.
func TestRevokeEndsWhatsOpen(t *testing.T) {
	var l liveConns
	ended := 0
	done := l.add("a", func() { ended++ })
	l.add("b", func() { t.Error("another device's ended") })
	if l.count("a") != 1 {
		t.Fatal(l.count("a"))
	}
	l.drop("a")
	done()
	if ended != 1 || l.count("a") != 0 || l.count("b") != 1 {
		t.Errorf("ended %d, a %d, b %d", ended, l.count("a"), l.count("b"))
	}
}
