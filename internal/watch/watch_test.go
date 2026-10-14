package watch

import (
	"context"
	"strconv"
	"strings"
	"testing"
	"time"
)

// script is a watch over polls, one PR per look, on a clock that moves
// only when it sleeps.
func script(t *testing.T, until Until, timeout time.Duration, polls ...PR) (int, []string, int) {
	t.Helper()
	var said []string
	now := time.Unix(0, 0)
	looks := 0
	code := WatchPR(context.Background(), Options{
		Repo: "o/r", Number: 7, Until: until, Timeout: timeout,
		Say: func(s string) { said = append(said, s) },
		Fetch: func(context.Context, string, int, time.Duration) (PR, int, error) {
			p := polls[min(looks, len(polls)-1)]
			looks++
			return p, 4000, nil
		},
		Sleep: func(_ context.Context, d time.Duration) bool { now = now.Add(d); return true },
		Logs: func(context.Context, string, int64) ([]byte, error) {
			return []byte("2026-10-14T10:00:00.0000000Z ##[group]Run go vet\n2026-10-14T10:00:00.0000000Z ##[endgroup]\n2026-10-14T10:00:01.0000000Z x.go:3:1: declared and not used: n\n2026-10-14T10:00:01.0000000Z ##[error]Process completed with exit code 1.\n"), nil
		},
		Now: func() time.Time { return now },
	})
	return code, said, looks
}

func pr(head string, checks ...string) PR {
	p := PR{Number: 7, Title: "t", State: "open", Head: head}
	for _, c := range checks {
		name, state, _ := strings.Cut(c, "=")
		p.Checks = append(p.Checks, Check{ID: 1, Name: name, State: state, Actions: true})
	}
	return p
}

func TestDoneWaitsForEveryCheckAndSaysOnce(t *testing.T) {
	code, said, looks := script(t, UntilDone, time.Hour,
		pr("a", "lint=pending", "test=pending"),
		pr("a", "lint=failed", "test=pending"),
		pr("a", "lint=failed", "test=passed"))
	if code != ExitFailed || looks != 3 || len(said) != 1 {
		t.Fatalf("code %d after %d looks, said %q", code, looks, said)
	}
	for _, want := range []string{"1 passed, 1 failed", "✗ lint failed", "declared and not used: n"} {
		if !strings.Contains(said[0], want) {
			t.Errorf("summary lacks %q:\n%s", want, said[0])
		}
	}
	if strings.Contains(said[0], "test") {
		t.Errorf("passes are named:\n%s", said[0])
	}
}

func TestFailEndsAtTheFirstFailure(t *testing.T) {
	code, _, looks := script(t, UntilFail, time.Hour,
		pr("a", "lint=pending", "test=pending"),
		pr("a", "lint=failed", "test=pending"))
	if code != ExitFailed || looks != 2 {
		t.Fatalf("code %d after %d looks", code, looks)
	}
}

func TestFirstEndsAtTheFirstToFinish(t *testing.T) {
	code, said, _ := script(t, UntilFirst, time.Hour,
		pr("a", "lint=pending", "test=pending"),
		pr("a", "lint=passed", "test=pending"))
	if code != ExitPassed || !strings.Contains(said[0], "1/2 finished") || !strings.Contains(said[0], "✓ lint passed") {
		t.Fatalf("code %d, said %q", code, said)
	}
}

func TestEachSaysEveryCheckThenSums(t *testing.T) {
	_, said, _ := script(t, UntilEach, time.Hour,
		pr("a", "lint=pending", "test=pending"),
		pr("a", "lint=passed", "test=pending"),
		pr("b", "lint=pending", "test=pending"),
		pr("b", "lint=passed", "test=passed"))
	got := strings.Join(said, "\n")
	for _, want := range []string{"✓ lint passed", "new commit b", "✓ test passed", "2 passed"} {
		if !strings.Contains(got, want) {
			t.Errorf("lacks %q:\n%s", want, got)
		}
	}
}

func TestTimeoutSaysHowFarItGot(t *testing.T) {
	code, said, _ := script(t, UntilDone, 2*time.Minute, pr("a", "lint=passed", "test=pending"))
	if code != ExitTimedOut || !strings.Contains(said[0], "1/2 finished") || !strings.Contains(said[0], "running: test") {
		t.Fatalf("code %d, said %q", code, said)
	}
}

func TestMergedEndsIt(t *testing.T) {
	p := pr("a", "lint=pending")
	p.State = "merged"
	if code, said, _ := script(t, UntilDone, time.Hour, p); code != ExitPassed || !strings.Contains(said[0], "merged") {
		t.Fatalf("code %d, said %q", code, said)
	}
}

func TestNoChecksEndsAfterAWhile(t *testing.T) {
	if code, said, _ := script(t, UntilDone, time.Hour, pr("a")); code != ExitPassed || !strings.Contains(said[0], "no checks") {
		t.Fatalf("code %d, said %q", code, said)
	}
}

func TestIntervalBacksOff(t *testing.T) {
	for _, c := range []struct {
		since time.Duration
		left  int
		want  time.Duration
	}{
		{0, -1, 15 * time.Second},
		{6 * time.Minute, 4000, 30 * time.Second},
		{20 * time.Minute, 4000, time.Minute},
		{0, 300, 2 * time.Minute},
		{0, 50, 5 * time.Minute},
	} {
		if got := Interval(c.since, c.left); got != c.want {
			t.Errorf("Interval(%s, %d) = %s, want %s", c.since, c.left, got, c.want)
		}
	}
}

func TestLogErrors(t *testing.T) {
	ts := "2026-10-14T10:00:00.1Z "
	log := ts + "##[group]Run actions/checkout@v4\n" + ts + "with: x\n" + ts + "##[endgroup]\n" +
		ts + " * [new branch]  fix/an-error-banner -> origin/fix/an-error-banner\n" +
		ts + "##[group]Run go test ./...\n" + ts + "go test ./...\n" + ts + "##[endgroup]\n" +
		ts + "ok  \tpkg/a\t0.1s\n" +
		ts + "\x1b[31m--- FAIL: TestX (0.00s)\x1b[0m\n" +
		ts + "    x_test.go:9: got 1, want 2\n" +
		ts + "FAIL\tpkg/b\t0.2s\n" +
		ts + "##[error]Process completed with exit code 1.\n" +
		ts + "##[group]Run pnpm format:check\n" + ts + "pnpm format:check\n" + ts + "##[endgroup]\n" +
		ts + "a.ts (0ms)\n" + ts + "Format issues found in above 1 files.\n" + ts + "[ELIFECYCLE] Command failed with exit code 1.\n" +
		ts + "##[error]Process completed with exit code 1.\n" +
		ts + "Post job cleanup. error noise\n"
	got := strings.Join(LogErrors(log, 8), "\n")
	want := "step: go test ./...\nok  \tpkg/a\t0.1s\n--- FAIL: TestX (0.00s)\nx_test.go:9: got 1, want 2\nFAIL\tpkg/b\t0.2s\n" +
		"step: pnpm format:check\na.ts (0ms)\nFormat issues found in above 1 files."
	if got != want {
		t.Fatalf("got\n%s\nwant\n%s", got, want)
	}
	// A log with no steps: what it said before it gave up.
	if got := LogErrors(ts+"make: *** [all] 2\n"+ts+"##[error]Process completed with exit code 2.\n", 3); len(got) != 1 || got[0] != "make: *** [all] 2" {
		t.Fatalf("got %q", got)
	}
}

func TestPickKeepsErrorsAndTheTail(t *testing.T) {
	lines := []string{"a", "error: one", "b", "c", "d", "e", "f"}
	if got := strings.Join(pick(lines, 4), ","); got != "error: one,d,e,f" {
		t.Fatalf("got %s", got)
	}
}

func TestParseResponseTakesTheLastAfterARedirect(t *testing.T) {
	out := "HTTP/2.0 302 Found\r\nLocation: x\r\n\r\nHTTP/2.0 200 OK\r\nEtag: W/\"e\"\r\nX-Ratelimit-Remaining: 42\r\n\r\n{\"a\":1}"
	status, h, body := parseResponse([]byte(out))
	if status != 200 || h["etag"] != `W/"e"` || h["x-ratelimit-remaining"] != "42" || string(body) != `{"a":1}` {
		t.Fatalf("%d %v %q", status, h, body)
	}
}

func TestGetSharesAndSendsTheETag(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	var asked [][]string
	run := Runner
	t.Cleanup(func() { Runner = run })
	Runner = func(_ context.Context, args ...string) ([]byte, []byte, error) {
		asked = append(asked, args)
		if len(args) > 1 {
			return []byte("HTTP/2.0 304 Not Modified\r\nX-Ratelimit-Remaining: 99\r\n\r\n"), []byte("gh: HTTP 304"), context.Canceled
		}
		return []byte("HTTP/2.0 200 OK\r\nEtag: \"e1\"\r\nX-Ratelimit-Remaining: 100\r\n\r\n{\"n\":1}"), nil, nil
	}
	ctx := context.Background()
	b, left, err := Get(ctx, "repos/o/r/pulls/1", time.Hour)
	if err != nil || string(b) != `{"n":1}` || left != 100 {
		t.Fatalf("%q %d %v", b, left, err)
	}
	// Fresh: another watcher's answer, with no call.
	if _, _, err := Get(ctx, "repos/o/r/pulls/1", time.Hour); err != nil || len(asked) != 1 {
		t.Fatalf("asked %d times, %v", len(asked), err)
	}
	// Stale: asked with the ETag; unchanged keeps the body.
	b, left, err = Get(ctx, "repos/o/r/pulls/1", 0)
	if err != nil || string(b) != `{"n":1}` || left != 99 || len(asked) != 2 || asked[1][1] != `If-None-Match: "e1"` {
		t.Fatalf("%q %d %v %q", b, left, err, asked)
	}
}

func TestResolveReadsURLsAndOwnerName(t *testing.T) {
	for in, want := range map[string]string{
		"https://github.com/0xdeafcafe/rush/pull/42": "0xdeafcafe/rush#42",
		"0xdeafcafe/rush#42":                         "0xdeafcafe/rush#42",
	} {
		repo, n, err := Resolve(context.Background(), "", in)
		if err != nil || repo+"#"+strconv.Itoa(n) != want {
			t.Errorf("Resolve(%q) = %s#%d, %v", in, repo, n, err)
		}
	}
}
