package remote

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/state"
)

// ErrMaybeStarted marks a start whose outcome isn't known: the request went
// out, but no whole answer came back (a timeout, a dropped link, a body that
// couldn't be read or understood), so the session may exist there.
var ErrMaybeStarted = errors.New("may or may not have started")

// StartOn asks peer p's serve to start a session: one POST /api/sessions,
// never sent again here. A failure with no answer (a timeout, a dropped
// link) may still have started one there; the caller says so rather than
// retry. The folder is the server's to read and check, never resolved here.
func StartOn(ctx context.Context, p Peer, r StartRequest) (Session, error) {
	var s Session
	body, _ := jsonx.Marshal(r)
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, p.URL+"/api/sessions", bytes.NewReader(body))
	if err != nil {
		return s, err
	}
	req.Header.Set("Authorization", "Bearer "+p.Token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return s, fmt.Errorf("no answer from %s, so it %w: %v", p.Name, ErrMaybeStarted, err)
	}
	defer resp.Body.Close()
	raw, rerr := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if resp.StatusCode >= 300 {
		var e struct{ Error string }
		_ = jsonx.Unmarshal(raw, &e)
		return s, fmt.Errorf("%s: %s", p.Name, or(e.Error, resp.Status))
	}
	if rerr == nil {
		rerr = jsonx.Unmarshal(raw, &s)
	}
	if rerr != nil { // it answered 2xx: the session is likely there
		return Session{}, fmt.Errorf("%s answered but its reply couldn't be read, so it %w: %v", p.Name, ErrMaybeStarted, rerr)
	}
	return s, nil
}

// Attached is whether a rush remote attach to machine is running here.
func Attached(machine string) bool {
	f, err := os.OpenFile(filepath.Join(state.Dir(), "attach-"+machine+".lock"), os.O_RDWR, 0)
	if err != nil {
		return false
	}
	defer f.Close()
	err = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	if err == nil {
		_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
		return false
	}
	return errors.Is(err, syscall.EWOULDBLOCK)
}
