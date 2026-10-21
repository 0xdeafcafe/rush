package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
)

// A switch because the login in use ran out leaves a session on another
// login with room where it is: moving it would only lose its cache.
func TestSwitchLeavesSessionsOnLoginsWithRoom(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	for id, login := range map[string]string{"on-alex": "alex-id", "on-personal": "personal-id"} {
		d := filepath.Join(host.Root(), id)
		if err := os.MkdirAll(d, 0o700); err != nil {
			t.Fatal(err)
		}
		b, _ := jsonx.Marshal(host.Info{ID: id, State: "idle", Homes: true, Login: login, HostPID: os.Getpid()})
		if err := os.WriteFile(filepath.Join(d, "info.json"), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	reloginHosts("claude", state.Config{}, map[string]string{"alex-id": "alex"})
	stays, _ := os.ReadFile(host.EventsPath("on-alex"))
	moves, _ := os.ReadFile(host.EventsPath("on-personal"))
	if !strings.Contains(string(stays), `"how":"stays"`) {
		t.Fatalf("the session on alex, with room, should stay:\n%s", stays)
	}
	if strings.Contains(string(moves), `"how":"stays"`) || !strings.Contains(string(moves), `"kind":"relogin"`) {
		t.Fatalf("the session on personal should be moved:\n%s", moves)
	}
}

// A session says the login it runs on by your name for it, even when new
// sessions run on another since a switch.
func TestSessionLoginNamesTheOneItRunsOn(t *testing.T) {
	m := &Model{snap: &fleet.Snapshot{}}
	m.snap.Logins = []fleet.LoginView{
		{Login: state.Login{ID: "a", Name: "personal", Email: "me@x"}, Current: true},
		{Login: state.Login{ID: "b", Name: "alex", Email: "alex@y"}},
	}
	c := &hostConn{sess: &convo.Session{}}
	c.sess.Info.Login = "b"
	lv, ok := m.sessionLogin(c)
	if !ok || loginName(lv) != "alex" || lv.Current || currentLoginName(m.snap.Logins) != "personal" {
		t.Fatalf("got %+v", lv)
	}
	c.sess.Info.Login = "0123456789"
	if lv, _ := m.sessionLogin(c); loginName(lv) != "01234567" {
		t.Fatalf("an unknown login is named by its id: %q", loginName(lv))
	}
	if _, ok := m.sessionLogin(nil); ok {
		t.Fatal("no session, no login")
	}
	if _, ok := m.sessionLogin(&hostConn{}); ok {
		t.Fatal("not read yet, no login")
	}
}
