package ui

import (
	tea "charm.land/bubbletea/v2"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/remote"
)

func TestOnStartWords(t *testing.T) {
	cfg := remote.Config{Peers: []remote.Peer{{Name: "box", URL: "http://x", Token: "t"}}}
	p, r, err := onStart(cfg, "box /srv/app agent=codex fix the build")
	if err != nil || p.Name != "box" || r.Cwd != "/srv/app" || r.Agent != "codex" || r.Prompt != "fix the build" {
		t.Fatalf("%+v %+v %v", p, r, err)
	}
	if _, r, _ = onStart(cfg, "box fix the build"); r.Cwd != "~" || r.Prompt != "fix the build" {
		t.Errorf("no folder: %+v", r) // the machine's ~, not a path here
	}
	for _, bad := range []string{"", "nope do it", "box", "box /srv/app"} {
		if _, _, err := onStart(cfg, bad); err == nil {
			t.Errorf("%q should be refused", bad)
		}
	}
}

// onRig is a model whose Prompt dispatch (submit) runs #on against fake
// machines; box answers by reply, after release when blocking.
type onRig struct {
	m         *Model
	hits, far atomic.Int32
	status    atomic.Int32
	body      atomic.Value
	release   chan struct{}
}

func newOnRig(t *testing.T, block bool) *onRig {
	t.Helper()
	t.Setenv("RUSH_HOME", t.TempDir())
	r := &onRig{release: make(chan struct{})}
	r.status.Store(http.StatusOK)
	r.body.Store(`{"id":"r1","name":"fix","alive":true}`)
	box := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, q *http.Request) {
		r.hits.Add(1)
		if block {
			<-r.release
		}
		w.WriteHeader(int(r.status.Load()))
		_, _ = w.Write([]byte(r.body.Load().(string)))
	}))
	t.Cleanup(box.Close)
	far := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { r.far.Add(1) }))
	t.Cleanup(far.Close)
	if err := remote.SaveConfig(remote.Config{Peers: []remote.Peer{
		{Name: "box", URL: box.URL, Token: "t"}, {Name: "far", URL: far.URL, Token: "t"}}}); err != nil {
		t.Fatal(err)
	}
	r.m, _ = benchModel(120, 40)
	return r
}

// enter types text in the Prompt and presses enter's path: submit, as the
// key does. It returns the command's result message, run off the UI.
func (r *onRig) enter(text string) func() tea.Msg {
	r.m.input, r.m.back, r.m.inKind = []rune(text), 0, inPrompt
	return r.m.submit()
}

// apply runs a command and feeds its message back, as the loop does.
func (r *onRig) apply(cmd func() tea.Msg) {
	if a, ok := cmd().(applyMsg); ok {
		a(r.m)
	}
}

func (r *onRig) box() string { return string(r.m.input) }

func TestOnDispatchValidationKeepsTheCommand(t *testing.T) {
	r := newOnRig(t, false)
	for _, bad := range []string{"#on nope do it", "#on box", "#on"} {
		if cmd := r.enter(bad); cmd != nil {
			t.Fatalf("%q: a command ran", bad)
		}
		if r.box() != bad {
			t.Errorf("%q came back as %q", bad, r.box())
		}
	}
	if r.hits.Load()+r.far.Load() != 0 {
		t.Error("something was sent")
	}
}

func TestOnDispatchRefusalAndAmbiguousComeBack(t *testing.T) {
	r := newOnRig(t, false)
	picked := startOver{kind: "claude"}
	r.m.startOver = &picked
	r.status.Store(http.StatusForbidden)
	r.body.Store(`{"error":"that folder is rush's own"}`)
	r.apply(r.enter("#on box /srv/app fix it"))
	if r.box() != "#on box /srv/app fix it" || r.m.onBusy || !strings.Contains(r.m.status, "rush's own") || r.hits.Load() != 1 {
		t.Fatalf("refusal: box %q busy %v status %q hits %d", r.box(), r.m.onBusy, r.m.status, r.hits.Load())
	}
	// a 200 whose body can't be read is not a failure to retry
	r.m.input = nil
	r.status.Store(http.StatusOK)
	r.body.Store(`not json`)
	r.apply(r.enter("#on box fix it"))
	if r.box() != "#on box fix it" || !strings.Contains(r.m.status, "may or may not") || r.hits.Load() != 2 {
		t.Fatalf("ambiguous: box %q status %q hits %d", r.box(), r.m.status, r.hits.Load())
	}
	if r.m.startOver != &picked || r.far.Load() != 0 {
		t.Error("local start options or the other machine were touched")
	}
	// success: nothing comes back
	r.m.input = nil
	r.body.Store(`{"id":"r1","name":"fix","alive":true}`)
	r.apply(r.enter("#on box fix it"))
	if r.box() != "" || !strings.Contains(r.m.status, "started fix on box") {
		t.Errorf("success: box %q status %q", r.box(), r.m.status)
	}
}

// While a start waits, a second #on is not sent, and what was typed since
// is never overwritten by a failure coming back.
func TestOnDispatchPendingIsNotDuplicatedNorOverwritten(t *testing.T) {
	r := newOnRig(t, true)
	r.status.Store(http.StatusForbidden)
	r.body.Store(`{"error":"no"}`)
	first := r.enter("#on box one")
	if !r.m.onBusy {
		t.Fatal("not pending")
	}
	done := make(chan tea.Msg, 1)
	go func() { done <- first() }()
	for r.hits.Load() == 0 {
		time.Sleep(5 * time.Millisecond)
	}
	if cmd := r.enter("#on box two"); cmd != nil {
		t.Fatal("a second start went out while one waits")
	}
	if r.box() != "#on box two" {
		t.Errorf("the second was lost: %q", r.box())
	}
	r.m.input = []rune("typed meanwhile")
	r.release <- struct{}{}
	r.apply(func() tea.Msg { return <-done })
	if r.box() != "typed meanwhile" || !strings.Contains(r.m.status, "#on box one") || r.m.onBusy || r.hits.Load() != 1 {
		t.Fatalf("box %q status %q busy %v hits %d", r.box(), r.m.status, r.m.onBusy, r.hits.Load())
	}
}
