package main

// Website evidence is still rendered by the installed rush. These are fixture
// records, not painted rows: a copy of this tool stands in for a live worker,
// and no model or real user's session is started.
import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"
)

func (w *world) website(question bool) error {
	config := filepath.Join(w.home, ".config", "rush", "config.json")
	data, err := os.ReadFile(config)
	if err != nil {
		return err
	}
	// Keep the full Agents table visible instead of the automatic split preview.
	data = append([]byte(`{"listOnly":true,"twatter":true,`), data[1:]...)
	if err := os.WriteFile(config, data, 0o644); err != nil {
		return err
	}
	// rush checks the process name as well as the PID. Use a copy of this
	// executable named claude, blocked on stdin, as the fleet tests do.
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	binary, err := os.ReadFile(executable)
	if err != nil {
		return err
	}
	workerPath := filepath.Join(w.root, "workers", "claude")
	if err := os.MkdirAll(filepath.Dir(workerPath), 0o755); err != nil {
		return err
	}
	if err := os.WriteFile(workerPath, binary, 0o755); err != nil {
		return err
	}
	var workers []any
	for _, job := range []struct{ id, sid, name, cwd, state, detail string }{
		{"5e1b7a3c", "5e1b7a3c-2d4f-4a8b-9c6d-1e2f3a4b5c6d", "Add Apple Pay to checkout", w.src("acme", "checkout", ".claude", "worktrees", "apple-pay"), "working", "checking the merchant validation"},
		{"b3a4c5d6", "b3a4c5d6-8e9f-4a0b-9c1d-2e3f4a5b6c7d", "Fix the flaky tax rounding test", w.src("acme", "checkout", ".claude", "worktrees", "tax-rounding"), "working", "reading the rounding tests"},
	} {
		if question && job.id == "b3a4c5d6" {
			job.state = "blocked"
			job.detail = "Round half up, or half to even?"
		}
		record := map[string]any{"state": job.state, "detail": job.detail, "name": job.name, "cwd": job.cwd, "sessionId": job.sid, "createdAt": w.ago(34 * time.Minute).Format(time.RFC3339), "updatedAt": w.now.Format(time.RFC3339)}
		if job.state == "blocked" {
			record["needs"] = job.detail
		}
		if err := write(filepath.Join(w.home, ".claude", "jobs", job.id), "state.json", compact(record)); err != nil {
			return err
		}
		worker := exec.Command(workerPath, "--fixture-worker")
		input, err := worker.StdinPipe()
		if err != nil {
			return err
		}
		if err := worker.Start(); err != nil {
			input.Close()
			return err
		}
		w.fixtures = append(w.fixtures, worker)
		w.fixtureInput = append(w.fixtureInput, input)
		workers = append(workers, map[string]any{"pid": worker.Process.Pid, "sessionId": job.sid, "dispatch": map[string]any{"short": job.id}})
	}
	if err := write(filepath.Join(w.home, ".claude", "daemon"), "roster.json", compact(map[string]any{"workers": workers})); err != nil {
		return err
	}
	var threads []any
	for i, post := range []struct{ name, handle, text string }{
		{"Move the design tokens", "move-the-design-tokens", "starting a full typecheck, the machine will crawl for a minute #build"},
		{"Rate limit the public API", "rate-limit-the-public-api", "who's running a build? my tests are crawling"},
		{"Move the design tokens", "move-the-design-tokens", "@rate-limit-the-public-api me, sorry. done in a minute"},
	} {
		at := w.ago(time.Duration(3-i) * time.Minute).Format(time.RFC3339)
		author := map[string]any{"name": post.name, "handle": post.handle, "kind": "claude", "project": w.src("acme", "checkout")}
		threads = append(threads, map[string]any{"id": fmt.Sprintf("fixture-%d", i), "title": post.text, "author": author, "createdAt": at, "updatedAt": at, "messages": []any{map[string]any{"author": author, "text": post.text, "at": at}}})
	}
	return write(filepath.Join(w.home, ".config", "rush", "community"), "board.json", compact(map[string]any{"version": 1, "threads": threads}))
}

func (w *world) stopWebsite() {
	for _, input := range w.fixtureInput {
		_ = input.Close()
	}
	for _, worker := range w.fixtures {
		_ = worker.Wait()
	}
}
