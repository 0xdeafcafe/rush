package ollama

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestInstalledModelsPreservesLoadedContextWithoutLoading(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/version":
			w.Write([]byte(`{"version":"test"}`))
		case "/api/tags":
			w.Write([]byte(`{"models":[{"name":"coder"},{"name":"embed"},{"name":"rush-context-hidden"}]}`))
		case "/api/ps":
			w.Write([]byte(`{"models":[{"name":"coder","context_length":8192,"size_vram":123}]}`))
		case "/api/show":
			var in struct{ Model string }
			jsonx.Decode(r.Body, &in)
			if in.Model == "coder" {
				w.Write([]byte(`{"capabilities":["tools"],"model_info":{"test.context_length":32768}}`))
			} else {
				w.Write([]byte(`{"capabilities":["embedding"]}`))
			}
		default:
			t.Errorf("discovery must not load models: %s", r.URL.Path)
			w.WriteHeader(500)
		}
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	models, err := InstalledModels(context.Background())
	if err != nil || len(models) != 2 {
		t.Fatalf("models=%v err=%v", models, err)
	}
	if models[0].Context != 8192 || models[0].MaxContext != 32768 {
		t.Fatalf("context lost: %+v", models[0])
	}
	if got := (Adapter{}).ContextWindow("coder"); got != 8192 {
		t.Fatalf("window=%d", got)
	}
}

func TestContextPreferenceCreatesPersistentModelAndWaitsForLoad(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	knownModels.Store("my-coder", Model{Name: "my-coder", MaxContext: 32768})
	defer knownModels.Delete("my-coder")
	if err := SetContextSize("my-coder", 65536); err == nil {
		t.Fatal("accepted above model maximum")
	}
	if err := SetContextSize("my-coder", 16384); err != nil {
		t.Fatal(err)
	}
	var alias string
	var created, loaded bool
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var in map[string]any
		jsonx.Decode(r.Body, &in)
		switch r.URL.Path {
		case "/api/create":
			created = true
			alias = in["model"].(string)
			if in["from"] != "my-coder" || in["parameters"].(map[string]any)["num_ctx"] != float64(16384) || in["stream"] != false {
				t.Errorf("create=%v", in)
			}
			w.Write([]byte(`{"status":"success"}`))
		case "/api/generate":
			if in["model"] != alias || in["stream"] != false {
				t.Errorf("load=%v", in)
			}
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
			time.Sleep(20 * time.Millisecond)
			loaded = true
			w.Write([]byte(`{"done":true}`))
		default:
			t.Errorf("unexpected %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	m, err := configuredModel(context.Background(), Model{Name: "my-coder", MaxContext: 32768})
	if err != nil || !created || !strings.HasPrefix(m.Name, "rush-context-") || m.Context != 16384 {
		t.Fatalf("model=%+v err=%v", m, err)
	}
	if err := load(context.Background(), m.Name); err != nil {
		t.Fatal(err)
	}
	if !loaded {
		t.Fatal("load returned before response body arrived")
	}
	if err := SetContextSize("my-coder", 0); err != nil || ContextSize("my-coder") != 0 {
		t.Fatal("reset failed", err)
	}
}

func TestEnsureRunningDoesNotStartOverExistingHTTPServer(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(http.StatusUnauthorized) }))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	if err := EnsureRunning(context.Background()); err != nil {
		t.Fatal(err)
	}
}

func TestEnsureRunningNeverStartsRemoteServer(t *testing.T) {
	t.Setenv("OLLAMA_HOST", "http://192.0.2.1:1")
	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	if err := EnsureRunning(ctx); err == nil {
		t.Fatal("remote unavailable should fail")
	}
}

func TestConfiguredModelNameStaysRecognizable(t *testing.T) {
	for _, id := range []string{"rush-context-qwen2.5:1.5b-ctx-16384", "ollama/rush-context-qwen2.5:1.5b-ctx-16384"} {
		if got := modelName(id); got != "qwen2.5:1.5b · 16384 context" {
			t.Errorf("%s became %s", id, got)
		}
	}
	if got := modelName("qwen3.8:27b-mlx"); got != "qwen3.8:27b-mlx" {
		t.Fatal(got)
	}
}
