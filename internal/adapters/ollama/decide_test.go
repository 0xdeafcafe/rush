package ollama

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/0xdeafcafe/rush/internal/jsonx"
)

func TestKeepAsksEachStepAloneInOrder(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	var mu sync.Mutex
	busy, most := 0, 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/systemone" {
			fmt.Fprint(w, `{}`)
			return
		}
		mu.Lock()
		busy++
		most = max(most, busy)
		mu.Unlock()
		defer func() { mu.Lock(); busy--; mu.Unlock() }()
		var in struct {
			State     string
			Questions map[string]struct{ Instructions string }
		}
		if err := jsonx.Decode(r.Body, &in); err != nil {
			t.Error(err)
			return
		}
		q, ok := in.Questions["keep"]
		// tev1 reads the state and every question as one prompt of at most
		// 2050 tokens; at 2 bytes a token, one step's request must fit.
		if !ok || len(in.Questions) != 1 || len(in.State)+len(q.Instructions) > 4000 {
			t.Errorf("%d questions, %d bytes", len(in.Questions), len(in.State)+len(q.Instructions))
		}
		step := strings.TrimPrefix(q.Instructions, "An earlier tool step:\n")
		id, _ := strconv.Atoi(strings.Fields(step)[0])
		if id == 999 {
			w.WriteHeader(http.StatusBadRequest)
			fmt.Fprint(w, `{"error":"prompt 0 has 2400 tokens; expected 1–2050"}`)
			return
		}
		time.Sleep(5 * time.Millisecond)
		jsonx.Write(w, map[string]any{"answers": map[string]any{"keep": map[string]any{"noul": float64(id) / 1000}}})
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	items := make([]string, 40)
	for i := range items {
		items[i] = fmt.Sprintf("%d %s", i, strings.Repeat("\"<\n", 3000))
	}
	got, err := Keep(context.Background(), "tev1", strings.Repeat("fix the build ", 1000), items)
	if err != nil {
		t.Fatal(err)
	}
	for i, p := range got {
		if p != float64(i)/1000 {
			t.Fatalf("score %d = %v: answers out of order", i, p)
		}
	}
	if most > decideParallel || most < 2 {
		t.Fatalf("%d at once", most)
	}
	items[7] = "999 too long"
	if _, err := Keep(context.Background(), "tev1", "x", items); err == nil || !strings.Contains(err.Error(), "2050") {
		t.Fatalf("err=%v", err)
	}
}

func TestPullReportsProgressAndOllamaErrors(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	fail := false
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/pull" {
			fmt.Fprint(w, `{}`)
			return
		}
		fmt.Fprintln(w, `{"status":"pulling","total":100,"completed":40}`)
		if fail {
			fmt.Fprintln(w, `{"error":"pull model manifest: file does not exist"}`)
			return
		}
		fmt.Fprintln(w, `{"status":"success"}`)
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	broken.Store("tev1", "missing blob")
	var done int64
	if err := Pull(context.Background(), "tev1", func(d, _ int64) { done = d }); err != nil || done != 40 {
		t.Fatalf("err=%v done=%d", err, done)
	}
	if _, ok := Broken()["tev1"]; ok {
		t.Fatal("a pulled model is still broken")
	}
	fail = true
	if err := Pull(context.Background(), "nope", nil); err == nil || !strings.Contains(err.Error(), "does not exist") {
		t.Fatalf("err=%v", err)
	}
}
