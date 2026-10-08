package ollama

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

var startMu sync.Mutex

// EnsureRunning starts a local server only when its API cannot be reached.
// Existing and remote servers are never restarted or reconfigured.
func EnsureRunning(ctx context.Context) error {
	startMu.Lock()
	defer startMu.Unlock()
	probe := func() error {
		pctx, cancel := context.WithTimeout(ctx, time.Second)
		defer cancel()
		req, err := http.NewRequestWithContext(pctx, http.MethodGet, server()+"/api/version", nil)
		if err != nil {
			return err
		}
		resp, err := client.Do(req)
		if err != nil {
			return err
		}
		resp.Body.Close()
		// Any HTTP responder already owns the endpoint; never spawn over it.
		return nil
	}
	if probe() == nil {
		return nil
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	u, err := url.Parse(server())
	if err != nil {
		return err
	}
	h := u.Hostname()
	ip := net.ParseIP(h)
	if h != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return fmt.Errorf("Ollama at %s is unavailable; start it on that machine", server())
	}
	bin := agent.Path(Kind)
	if bin == "" {
		bin, err = exec.LookPath("ollama")
		if err != nil {
			return fmt.Errorf("install Ollama to start local models: %w", err)
		}
	}
	cmd := exec.Command(bin, "serve")
	// The shared local server must outlive the session that first needs it.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	cmd.Env = os.Environ()
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("start Ollama: %w", err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	timer := time.NewTimer(20 * time.Second)
	defer timer.Stop()
	tick := time.NewTicker(200 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-timer.C:
			return fmt.Errorf("Ollama started but its API is not ready at %s", server())
		case err := <-done:
			if probe() == nil {
				return nil
			}
			return fmt.Errorf("Ollama exited before becoming ready: %v", err)
		case <-tick.C:
			if probe() == nil {
				return nil
			}
		}
	}
}

var contextPrefs struct {
	sync.Mutex
	path   string
	values map[string]int
}
var knownModels sync.Map
var installedCatalog atomic.Pointer[[]Model]

// CachedInstalledModels returns an immutable discovery snapshot; no I/O.
func CachedInstalledModels() []Model {
	if p := installedCatalog.Load(); p != nil {
		return *p
	}
	return nil
}

func readContextsLocked() {
	path := filepath.Join(state.Dir(), "ollama-context.json")
	if contextPrefs.path == path {
		return
	}
	contextPrefs.path, contextPrefs.values = path, map[string]int{}
	if b, err := os.ReadFile(path); err == nil {
		_ = jsonx.Unmarshal(b, &contextPrefs.values)
	}
	if contextPrefs.values == nil {
		contextPrefs.values = map[string]int{}
	}
}

// ContextSize is the cached requested context, zero for the server default.
// Preferences are read while discovering models, off the UI.
func ContextSize(model string) int {
	contextPrefs.Lock()
	defer contextPrefs.Unlock()
	return contextPrefs.values[model]
}

// SetContextSize saves a preference for future sessions, without reloading
// any running model. A zero size restores the server default.
func SetContextSize(model string, tokens int) error {
	if model == "" || tokens < 0 || tokens > 16*1024*1024 || (tokens > 0 && tokens < 2048) {
		return fmt.Errorf("context must be 0 (automatic), or 2048–16777216 tokens")
	}
	if m, ok := CachedModel(model); ok && tokens > m.MaxContext && m.MaxContext > 0 {
		return fmt.Errorf("%s supports at most %d context tokens", model, m.MaxContext)
	}
	contextPrefs.Lock()
	defer contextPrefs.Unlock()
	readContextsLocked()
	next := make(map[string]int, len(contextPrefs.values)+1)
	for k, v := range contextPrefs.values {
		next[k] = v
	}
	if tokens == 0 {
		delete(next, model)
	} else {
		next[model] = tokens
	}
	if err := os.MkdirAll(filepath.Dir(contextPrefs.path), 0o755); err != nil {
		return err
	}
	if err := state.WriteJSON(contextPrefs.path, next); err != nil {
		return err
	}
	contextPrefs.values = next
	return nil
}

// CachedModel is safe in a view: no disk or network access.
func CachedModel(name string) (Model, bool) {
	v, ok := knownModels.Load(name)
	if !ok {
		return Model{}, false
	}
	return v.(Model), true
}

// InstalledModels includes models that cannot call tools so the UI can
// explain why they cannot run a coding harness. It never loads a model.
func InstalledModels(ctx context.Context) ([]Model, error) {
	if err := EnsureRunning(ctx); err != nil {
		return nil, err
	}
	contextPrefs.Lock()
	readContextsLocked()
	contextPrefs.Unlock()
	all, err := names(ctx)
	if err != nil {
		return nil, err
	}
	in, _ := loaded(ctx)
	out := make([]Model, 0, len(all))
	for _, name := range all {
		if strings.HasPrefix(name, "rush-context-") {
			continue
		}
		m, err := show(ctx, name)
		if err != nil {
			if ctx.Err() == nil {
				broken.Store(name, err.Error())
			}
			continue
		}
		broken.Delete(name)
		m.Context, m.VRAM = in[name].Context, in[name].VRAM
		knownModels.Store(name, m)
		out = append(out, m)
	}
	installedCatalog.Store(&out)
	return out, nil
}

// configuredModel persists num_ctx in a derived model: a preload option
// alone would be lost when a harness makes its next API request.
func configuredModel(ctx context.Context, m Model) (Model, error) {
	contextPrefs.Lock()
	readContextsLocked()
	n := contextPrefs.values[m.Name]
	contextPrefs.Unlock()
	if n == 0 {
		return m, nil
	}
	if m.MaxContext > 0 && n > m.MaxContext {
		return Model{}, fmt.Errorf("context %d exceeds %s's maximum %d", n, m.Name, m.MaxContext)
	}
	name := "rush-context-" + m.Name + "-ctx-" + strconv.Itoa(n)
	var result struct{ Status string }
	if err := call(ctx, http.MethodPost, "/api/create", map[string]any{"model": name, "from": m.Name, "parameters": map[string]int{"num_ctx": n}, "stream": false}, &result); err != nil {
		return Model{}, err
	}
	m.Name, m.Context = name, n
	knownModels.Store(name, m)
	return m, nil
}

func cachedWindow(name string) int64 {
	if m, ok := CachedModel(name); ok {
		return int64(window(m))
	}
	return 0
}
func (Adapter) ContextWindow(name string) int64      { return cachedWindow(name) }
func (CodexAdapter) ContextWindow(name string) int64 { return cachedWindow(name) }
func (PiAdapter) ContextWindow(name string) int64 {
	return cachedWindow(strings.TrimPrefix(name, "ollama/"))
}
func (VibeAdapter) ContextWindow(name string) int64 { return cachedWindow(name) }

// modelName keeps context-specific backing models recognizable in sessions.
func modelName(id string) string {
	id = strings.TrimPrefix(id, "ollama/")
	if base, ok := strings.CutPrefix(id, "rush-context-"); ok {
		if at := strings.LastIndex(base, "-ctx-"); at >= 0 {
			return base[:at] + " · " + base[at+5:] + " context"
		}
	}
	return id
}
func (Adapter) ModelName(id string) string      { return modelName(id) }
func (CodexAdapter) ModelName(id string) string { return modelName(id) }
func (PiAdapter) ModelName(id string) string    { return modelName(id) }
func (VibeAdapter) ModelName(id string) string  { return modelName(id) }
