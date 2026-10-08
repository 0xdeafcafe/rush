package ui

import (
	"sync/atomic"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/uithread"
)

// In tests, work handed off the UI goroutine happens at once: a frame
// drawn or a key pressed in a test has what a later frame would.
func init() {
	goOff = func(f func()) { uithread.Off(f) }
	cmdOff = func(f func() tea.Msg) tea.Cmd {
		var msg tea.Msg
		uithread.Off(func() { msg = f() })
		return func() tea.Msg { return msg }
	}
	uithread.Breach = offUIBreach
}

// For real, a read happens in the background: the frame draws what it had
// until the read is taken, and redraw lands once it's done.
func TestOffReadInBackground(t *testing.T) {
	was := goOff
	goOff = func(f func()) { go f() }
	defer func() { goOff = was }()

	var r offRead[int]
	release := make(chan struct{})
	var ran atomic.Int32
	if !r.start(func() int { ran.Add(1); <-release; return 7 }) {
		t.Fatal("the first read should start")
	}
	if r.start(func() int { ran.Add(1); return 8 }) {
		t.Fatal("a second read started while one was out")
	}
	if _, ok := r.take(); ok || !r.out() {
		t.Fatal("taken before it was done")
	}
	wait := r.redraw()
	close(release)
	if msg := wait(); msg == nil {
		t.Fatal("redraw should land")
	}
	deadline := time.Now().Add(time.Second)
	v, ok := r.take()
	for !ok && time.Now().Before(deadline) {
		v, ok = r.take()
	}
	if !ok || v != 7 || r.out() || ran.Load() != 1 {
		t.Fatalf("took %d %v, out %v, ran %d", v, ok, r.out(), ran.Load())
	}
	if _, ok := r.take(); ok {
		t.Fatal("taken twice")
	}
}
