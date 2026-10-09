package remote

import (
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/host"
)

// TestBrowserFixture is not a test: with RUSH_FIXTURE=<dir> it serves the
// web app on 127.0.0.1:<RUSH_FIXTURE_PORT> over fake hosts in an isolated
// home, with the ops they receive logged to <dir>/ops.log, until killed. A
// browser driver then exercises the controls with no real agent behind them.
// Token: "fixture" + 57 x. Sessions: one with an approval and a question
// pending, one that has nothing, and 9e9e9e9e: stopped, in a git repository
// with a.txt changed and new.txt untracked.
func TestBrowserFixture(t *testing.T) {
	dir := os.Getenv("RUSH_FIXTURE")
	if dir == "" {
		t.Skip("a fixture for browser runs: RUSH_FIXTURE=dir")
	}
	shortHome(t)
	a := startFakeHost(t, host.Root(), "a1a1a1a1", "needs you", "/work/a", approvalLine(t, "ap1"), questionLine(t, "q1"))
	b := startFakeHost(t, host.Root(), "b2b2b2b2", "quiet", "/work/b")
	gitRepo(t) // session 9e9e9e9e, stopped, in a repository with changes
	log, _ := os.Create(dir + "/ops.log")
	for _, f := range []*fakeHost{a, b} {
		go func() {
			for l := range f.ops {
				log.WriteString(f.id + " " + string(l) + "\n")
			}
		}()
	}
	// a's info says it's blocked, as a host waiting on a decision would
	a.info.State, a.info.Needs = "blocked", "approval"
	a.writeInfo(t, filepath.Join(host.Root(), "a1a1a1a1"))
	tok := "fixture" + strings.Repeat("x", 57)
	s := &Server{Config: Config{Name: "fixture"}, Token: tok, Start: func(r StartRequest) (host.Info, error) {
		raw, _ := jsonx.Marshal(r)
		log.WriteString("start " + string(raw) + "\n")
		return b.info, nil
	}}
	ln, err := net.Listen("tcp", "127.0.0.1:"+os.Getenv("RUSH_FIXTURE_PORT"))
	if err != nil {
		t.Fatal(err)
	}
	go http.Serve(ln, s.Handler())
	os.WriteFile(dir+"/ready", nil, 0o600)
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, os.Interrupt)
	<-sig
}
