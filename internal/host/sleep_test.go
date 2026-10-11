package host

import (
	"os"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/event"
)

func TestWakeKeepsSameSessionCountersAndPermissionChoices(t *testing.T) {
	old := Info{Sleeping: true, SessionID: "same", Kind: "kimi", PermissionMode: "ask", PermissionModes: []event.PermissionMode{{ID: "ask", Name: "Ask"}}, CostUSD: 1.25, ContextTokens: 900, CacheWarm: time.Now().Add(time.Hour), Billing: "api", QueueHeld: true, QueueSeparate: true, StartedAt: time.Unix(1, 0), ReplayFrom: time.Unix(2, 0)}
	for _, same := range []bool{true, false} {
		s := &server{cfg: Config{Resume: true, SessionID: "same"}, info: Info{Kind: "kimi", SessionID: "same", StartedAt: time.Now()}}
		if !same {
			s.cfg.SessionID = "fork"
		}
		s.restoreSleepingInfo(old)
		if same && (s.info.CostUSD != 1.25 || s.info.PermissionMode != "ask" || s.info.ContextTokens != 900 || len(s.info.PermissionModes) != 1 || !s.info.QueueHeld || !s.info.QueueSeparate || s.info.Billing != "api") {
			t.Fatalf("lost conversation metadata: %+v", s.info)
		}
		if !same && (s.info.CostUSD != 0 || len(s.info.PermissionModes) != 0) {
			t.Fatal("copied old session metadata to fork")
		}
		if s.info.Sleeping || !s.info.ReplayFrom.IsZero() || s.info.StartedAt == old.StartedAt {
			t.Fatal("restored transient sleep/replay state")
		}
	}
}

// A session restarted while idle keeps when it went idle (Serve restores it), and
// detail it publishes while idle doesn't move it: the list shows no finish.
func TestIdleSinceSurvivesWakeAndPublish(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	went := time.Unix(1000, 0)
	s := &server{cfg: Config{ID: "idle", Resume: true, SessionID: "same"}, began: true, clients: map[*conn]struct{}{}, info: Info{Kind: "codex", SessionID: "same", State: "idle"}}
	if err := os.MkdirAll(dir(s.cfg.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	s.info.IdleSince = went // as Serve restores it
	s.publish()
	s.info.Detail = "changed"
	s.publish()
	if !s.info.IdleSince.Equal(went) {
		t.Fatalf("idle since %v, want %v", s.info.IdleSince, went)
	}
	s.info.State = "working"
	s.publish()
	s.info.State = "idle"
	s.publish()
	if !s.info.IdleSince.After(went) {
		t.Fatal("a turn's end didn't move IdleSince")
	}
}
