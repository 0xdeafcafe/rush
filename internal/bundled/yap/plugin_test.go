package yap

import (
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestFollowReadsYapAndEndsWithIt(t *testing.T) {
	home, err := os.MkdirTemp("", "yap")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(home) })
	t.Setenv("HOME", home)
	if err := os.MkdirAll(filepath.Dir(Socket()), 0o700); err != nil {
		t.Fatal(err)
	}
	l, err := net.Listen("unix", Socket())
	if err != nil {
		t.Skip("no unix socket here:", err)
	}
	defer l.Close()
	go func() {
		c, err := l.Accept()
		if err != nil {
			return
		}
		c.Write([]byte(`{"phase":"listening","settled":"hello","guessing":"wor","level":40}` + "\n" + "not json\n"))
		time.Sleep(50 * time.Millisecond)
		c.Close() // yap quits mid-dictation
	}()

	got := make(chan State, 8)
	stop := make(chan struct{})
	defer close(stop)
	go Follow(stop, func(s State) { got <- s })

	want := []State{{Phase: "listening", Settled: "hello", Guessing: "wor", Level: 40}, {Phase: "hidden"}}
	for _, w := range want {
		select {
		case s := <-got:
			if s != w {
				t.Fatalf("got %+v, want %+v", s, w)
			}
		case <-time.After(2 * time.Second):
			t.Fatalf("nothing came, waiting for %+v", w)
		}
	}
}
