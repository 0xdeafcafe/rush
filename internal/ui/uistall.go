package ui

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/uithread"
	"github.com/0xdeafcafe/rush/internal/state"
)

// --- the UI's own goroutine, watched ---

// Nothing that waits (the disk, the network, another process) belongs on
// the UI's goroutine: every key waits behind it. A watchdog notices when a
// message or a frame holds it longer than stallAfter, and keeps what it was
// doing, the stack included, in stalls.log, so whatever still does is found
// by name rather than by feel. The Network sheet shows the count.

const stallAfter = 40 * time.Millisecond

// uiStall is one time the UI's goroutine was held up.
type uiStall struct {
	what  string // the message's type, or "frame"
	took  time.Duration
	at    time.Time
	where string // the deepest of rush's own calls it was in, when caught
}

var stalls struct {
	sync.Mutex
	on       bool
	began    time.Time // when the message or frame being handled began; zero between
	what     any
	caught   bool   // this one's stack is kept already
	where    string // where it was caught
	n        int
	longest  uiStall
	last     uiStall
	logPath  string
	watching bool
}

// uiBusy marks the UI's goroutine busy with what (a message, or "frame"
// for drawing one) until the returned func is called.
func uiBusy(what any) func() {
	leave := uithread.Enter()
	stalls.Lock()
	if !stalls.on {
		stalls.Unlock()
		return leave
	}
	began := time.Now()
	stalls.began, stalls.what, stalls.caught, stalls.where = began, what, false, ""
	stalls.Unlock()
	return func() {
		leave()
		took := time.Since(began)
		stalls.Lock()
		defer stalls.Unlock()
		stalls.began = time.Time{}
		if took < stallAfter {
			return
		}
		s := uiStall{what: stallName(stalls.what), took: took, at: began, where: stalls.where}
		stalls.n++
		stalls.last = s
		if took > stalls.longest.took {
			stalls.longest = s
		}
		if !stalls.caught {
			// Too quick for the watchdog to catch it at it: noted all the same.
			go appendStall(fmt.Sprintf("%s  %s took %s\n\n", began.Format(time.DateTime), s.what, took.Round(time.Millisecond)))
		}
	}
}

// offUIBreach is what ran on the UI's goroutine that never should: a test
// fails on it; a real run keeps its stack in stalls.log, so it's found by
// name however quick it was this time.
func offUIBreach(what string) {
	if testing.Testing() {
		panic(what + " ran on the UI's goroutine: hand it off (goOff, later, offRead)")
	}
	stack := make([]byte, 64<<10)
	stack = stack[:runtime.Stack(stack, false)]
	go appendStall(fmt.Sprintf("%s  %s ran on the UI's goroutine\n%s\n\n", time.Now().Format(time.DateTime), what, stack))
}

// watchUI starts the watchdog; it's safe to call more than once.
func watchUI() {
	stalls.Lock()
	defer stalls.Unlock()
	if stalls.watching {
		return
	}
	stalls.on, stalls.watching = true, true
	uithread.Breach = offUIBreach
	stalls.logPath = filepath.Join(state.Dir(), "stalls.log")
	go func() {
		for range time.Tick(stallAfter / 3) {
			stalls.Lock()
			began, what, caught := stalls.began, stalls.what, stalls.caught
			stalls.Unlock()
			if began.IsZero() || caught || time.Since(began) < stallAfter {
				continue
			}
			stack := uiStack()
			where := stallWhere(stack)
			stalls.Lock()
			if stalls.began.Equal(began) {
				stalls.caught, stalls.where = true, where
			}
			stalls.Unlock()
			appendStall(fmt.Sprintf("%s  %s held the UI over %s, in %s\n%s\n", began.Format(time.DateTime), stallName(what), stallAfter, where, stack))
		}
	}()
}

// uiStack is the stack of the goroutine running the UI's Update or View.
func uiStack() string {
	buf := make([]byte, 1<<20)
	buf = buf[:runtime.Stack(buf, true)]
	for g := range bytes.SplitSeq(buf, []byte("\n\n")) {
		if bytes.Contains(g, []byte("ui.(*Model).Update(")) || bytes.Contains(g, []byte("ui.(*Model).View(")) {
			return string(g)
		}
	}
	return ""
}

// stallWhere is the deepest of rush's own calls in a stack, past the
// runtime's and the standard library's: what was doing the waiting.
func stallWhere(stack string) string {
	for l := range strings.SplitSeq(stack, "\n") {
		if strings.HasPrefix(l, "\t") || !strings.Contains(l, "rush/internal/") {
			continue
		}
		fn := l[strings.LastIndex(l, "/")+1:]
		if i := strings.LastIndex(fn, "("); i > 0 {
			fn = fn[:i]
		}
		return fn
	}
	return "?"
}

func stallName(what any) string {
	if s, ok := what.(string); ok {
		return s
	}
	return strings.TrimPrefix(fmt.Sprintf("%T", what), "ui.")
}

// appendStall adds to stalls.log, starting it afresh once it's grown past
// a megabyte.
func appendStall(s string) {
	stalls.Lock()
	p := stalls.logPath
	stalls.Unlock()
	if p == "" {
		return
	}
	flag := os.O_CREATE | os.O_WRONLY | os.O_APPEND
	if st, err := os.Stat(p); err == nil && st.Size() > 1<<20 {
		flag |= os.O_TRUNC
	}
	f, err := os.OpenFile(p, flag, 0o600)
	if err != nil {
		return
	}
	defer f.Close()
	_, _ = f.WriteString(s)
}

// stallReport is what the watchdog has seen, for the Network sheet.
func stallReport() (n int, longest, last uiStall, path string) {
	stalls.Lock()
	defer stalls.Unlock()
	return stalls.n, stalls.longest, stalls.last, stalls.logPath
}
