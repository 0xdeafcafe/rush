package ollama

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"sync"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Decision models (tev1, nimble) answer typed questions over
// /v1/systemone with a probability instead of writing text. Rush asks
// them which tool steps a conversation still needs, and keeps those
// word for word: compaction by deleting, not summarising.

// DecisionModels are the installed models that answer decisions.
func DecisionModels(ctx context.Context) ([]string, error) {
	models, err := InstalledModels(ctx)
	if err != nil {
		return nil, err
	}
	var out []string
	for _, m := range models {
		if m.Can("decision") {
			out = append(out, m.Name)
		}
	}
	return out, nil
}

// CachedDecisionModel is the first decision model the last discovery
// found, or ""; no I/O.
func CachedDecisionModel() string {
	for _, m := range CachedInstalledModels() {
		if m.Can("decision") {
			return m.Name
		}
	}
	return ""
}

// tev1 reads at most 2050 tokens a prompt, and a request's every
// question is read with the state and all the others: so one step a
// request, the work in short as its state, a few at once.
// shortcut: bytes stand in for tokens at 2 a token, the worst code and
// JSON come to; read the limit off Ollama's error if a model's is smaller.
const (
	decideItem     = 1200
	decideGoal     = 2000
	decideParallel = 4 // about 3× one at a time on an M-series Mac
)

// Keep scores each item for whether the work in goal still needs it:
// the probability, 0 to 1, that it does, in the order given.
func Keep(ctx context.Context, model, goal string, items []string) ([]float64, error) {
	if err := EnsureRunning(ctx); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	state := "Current work:\n" + clipBytes(goal, decideGoal)
	out := make([]float64, len(items))
	slots := make(chan struct{}, decideParallel)
	var wg sync.WaitGroup
	for i, it := range items {
		slots <- struct{}{}
		if ctx.Err() != nil {
			break
		}
		wg.Go(func() {
			defer func() { <-slots }()
			var resp struct {
				Answers map[string]struct {
					Noul *float64 `json:"noul"`
				}
			}
			req := map[string]any{"model": model, "state": state, "questions": map[string]any{"keep": map[string]any{
				"type":         "noul",
				"instructions": "An earlier tool step:\n" + clipBytes(it, decideItem) + "\n\nDoes the current work still need this step's exact input or output to carry on?",
				"criteria": map[string]string{
					"false": "Finished, superseded, or easily redone: it can be dropped.",
					"true":  "Holds a path, error, value or decision the work still relies on.",
				},
			}}}
			if err := call(ctx, http.MethodPost, "/v1/systemone", req, &resp); err != nil {
				cancel(err)
				return
			}
			a := resp.Answers["keep"]
			if a.Noul == nil {
				cancel(fmt.Errorf("%s didn't answer for step %d", model, i+1))
				return
			}
			out[i] = *a.Noul
		})
	}
	wg.Wait()
	if err := context.Cause(ctx); err != nil {
		return nil, err
	}
	return out, nil
}

func clipBytes(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return strings.ToValidUTF8(s[:n], "") + " …"
}

// Pull downloads name, telling progress the bytes done of the total as
// they come. A model pulled again is mended: its missing files fetched.
func Pull(ctx context.Context, name string, progress func(done, total int64)) error {
	if err := EnsureRunning(ctx); err != nil {
		return err
	}
	body, err := jsonx.Marshal(map[string]any{"model": name, "stream": true})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, server()+"/api/pull", strings.NewReader(string(body)))
	if err != nil {
		return err
	}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("Ollama isn't running at %s: start it with `ollama serve`, or open the Ollama app", server())
	}
	defer resp.Body.Close()
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 64<<10), 1<<20)
	for sc.Scan() {
		var line struct {
			Status, Error    string
			Total, Completed int64
		}
		if jsonx.Unmarshal(sc.Bytes(), &line) != nil {
			continue
		}
		if line.Error != "" {
			return fmt.Errorf("ollama: %s", line.Error)
		}
		if progress != nil && line.Total > 0 {
			progress(line.Completed, line.Total)
		}
		if line.Status == "success" {
			broken.Delete(name)
			return nil
		}
	}
	if err := sc.Err(); err != nil {
		return err
	}
	return errors.New("ollama: pull ended before it finished")
}

// Remove deletes model name from Ollama.
func Remove(ctx context.Context, name string) error {
	if err := call(ctx, http.MethodDelete, "/api/delete", map[string]string{"model": name}, nil); err != nil {
		return err
	}
	broken.Delete(name)
	if p := installedCatalog.Load(); p != nil {
		next := make([]Model, 0, len(*p))
		for _, m := range *p {
			if m.Name != name {
				next = append(next, m)
			}
		}
		installedCatalog.Store(&next)
	}
	return nil
}

// broken are models Ollama lists but can't read: name → why. A pull
// mends one.
var broken sync.Map

// Broken are the installed models the last discovery couldn't read,
// each with why; no I/O.
func Broken() map[string]string {
	out := map[string]string{}
	broken.Range(func(k, v any) bool {
		out[k.(string)] = v.(string)
		return true
	})
	return out
}
