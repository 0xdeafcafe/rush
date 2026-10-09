package remote

import (
	"bufio"
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/host"
)

// countAgent's transcript is a number: that many messages.
type countAgent struct{}

func init() { agent.Register(countAgent{}) }

func (countAgent) Kind() agent.Kind                          { return "count" }
func (countAgent) Name() string                              { return "count" }
func (countAgent) Features() map[agent.Feature]agent.Support { return nil }
func (countAgent) Level() agent.Level                        { return 0 }
func (countAgent) Profiles() []agent.Profile                 { return nil }
func (countAgent) TranscriptPath(_ agent.Profile, cwd, sid string) string {
	return filepath.Join(cwd, sid)
}
func (countAgent) History(s agent.Session, _ time.Time) ([]event.Event, error) {
	b, err := os.ReadFile(s.Transcript)
	if err != nil {
		return nil, err
	}
	n, _ := strconv.Atoi(strings.TrimSpace(string(b)))
	out := make([]event.Event, n)
	for i := range out {
		out[i] = event.Message{Role: "assistant", Parts: []event.Part{{Kind: event.Text, Text: strconv.Itoa(i)}}}
	}
	return out, nil
}

// stream reads a session's events until it has a message saying last.
func stream(t *testing.T, url, last string) (msgs int, cut bool) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET", url, nil)
	req.Header.Set("Authorization", "Bearer "+strings.Repeat("a", 64))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	sc := bufio.NewScanner(resp.Body)
	for sc.Scan() {
		data, ok := strings.CutPrefix(sc.Text(), "data: ")
		if !ok {
			continue
		}
		if data == `{"t":"cut"}` {
			cut = true
			continue
		}
		var w struct {
			T string `json:"t"`
			E struct{ Parts []event.Part }
		}
		if jsonx.Unmarshal([]byte(data), &w) == nil && w.T == "message" {
			msgs++
			if w.E.Parts[0].Text == last {
				return msgs, cut
			}
		}
	}
	t.Fatalf("the stream ended after %d messages", msgs)
	return
}

func TestHistoryTailThenAll(t *testing.T) {
	s := newServer(t)
	cwd := t.TempDir()
	if err := os.WriteFile(filepath.Join(cwd, "sid"), []byte("1000"), 0o600); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(host.Root(), "c0c0c0c0")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	b, _ := jsonx.Marshal(host.Info{ID: "c0c0c0c0", SessionID: "sid", Kind: "count", Cwd: cwd, State: "stopped", UpdatedAt: time.Now()})
	if err := os.WriteFile(filepath.Join(dir, "info.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
	ts := httptest.NewServer(s.Handler())
	defer ts.Close()
	if n, cut := stream(t, ts.URL+"/api/sessions/c0c0c0c0/events", "999"); n != historyMost || !cut {
		t.Errorf("tail: %d messages, cut %v", n, cut)
	}
	if n, cut := stream(t, ts.URL+"/api/sessions/c0c0c0c0/events?full=1", "999"); n != 1000 || cut {
		t.Errorf("full: %d messages, cut %v", n, cut)
	}
}
