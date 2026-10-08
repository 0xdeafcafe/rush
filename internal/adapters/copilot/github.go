package copilot

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// token is the GitHub token Copilot is reached with: the one in the
// environment, else gh's for the account rush uses for Copilot.
func token() (string, error) {
	for _, k := range []string{"COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"} {
		if v := os.Getenv(k); v != "" {
			return v, nil
		}
	}
	return tokenFor(chosen())
}

// tokenFor is gh's token for login (its active account when empty), asked
// for at most every ten minutes.
func tokenFor(login string) (string, error) {
	tokenMu.Lock()
	defer tokenMu.Unlock()
	if t, ok := tokens[login]; ok && time.Since(t.at) < 10*time.Minute {
		return t.val, nil
	}
	args := []string{"auth", "token", "-h", "github.com"}
	if login != "" {
		args = append(args, "-u", login)
	}
	out, err := exec.Command(gh(), args...).Output()
	if err != nil {
		if login != "" {
			return "", errors.New("copilot: gh isn't signed in as " + login + " (gh auth login)")
		}
		return "", errors.New("copilot: not signed in to GitHub (gh auth login)")
	}
	tokens[login] = tokenEntry{strings.TrimSpace(string(out)), time.Now()}
	return tokens[login].val, nil
}

type tokenEntry struct {
	val string
	at  time.Time
}

var (
	tokenMu sync.Mutex
	tokens  = map[string]tokenEntry{}
)

// forgetTokens drops the tokens asked for, so the next ask is fresh.
func forgetTokens() {
	tokenMu.Lock()
	tokens = map[string]tokenEntry{}
	tokenMu.Unlock()
}

// gh is gh, wherever it's installed.
func gh() string {
	if p, ok := agent.Find("gh"); ok {
		return p
	}
	return "gh"
}

var client = &http.Client{Timeout: 20 * time.Second}

// get reads url's JSON into v, with the GitHub token.
func get(ctx context.Context, url string, v any) error {
	body, err := fetch(ctx, url)
	if err != nil {
		return err
	}
	defer body.Close()
	return jsonx.Decode(io.LimitReader(body, 16<<20), v)
}

// fetch is url's body, with the GitHub token.
func fetch(ctx context.Context, url string) (io.ReadCloser, error) {
	tok, err := token()
	if err != nil {
		return nil, err
	}
	return fetchWith(ctx, url, tok)
}

// fetchWith is url's body, with tok.
func fetchWith(ctx context.Context, url, tok string) (io.ReadCloser, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+tok)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Copilot-Integration-Id", "copilot-4-cli")
	req.Header.Set("User-Agent", "rush")
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != http.StatusOK {
		resp.Body.Close()
		return nil, fmt.Errorf("copilot: %s answered %d", req.URL.Path, resp.StatusCode)
	}
	return resp.Body, nil
}

// user is what GitHub says about your Copilot: your plan, your limits,
// and where its API is.
type user struct {
	Plan      string `json:"copilot_plan"`
	ResetsAt  string `json:"quota_reset_date_utc"`
	Endpoints struct {
		API string `json:"api"`
	} `json:"endpoints"`
	Quotas map[string]struct {
		Entitlement float64 `json:"entitlement"`
		Remaining   float64 `json:"remaining"`
		Percent     float64 `json:"percent_remaining"`
		Unlimited   bool    `json:"unlimited"`
		Overage     bool    `json:"overage_permitted"`
	} `json:"quota_snapshots"`
}

// readUser asks GitHub about your Copilot.
func readUser(ctx context.Context) (user, error) {
	var u user
	err := get(ctx, "https://api.github.com/copilot_internal/user", &u)
	return u, err
}

// readUserAs asks GitHub about login's Copilot.
func readUserAs(ctx context.Context, login string) (user, error) {
	tok, err := tokenFor(login)
	if err != nil {
		return user{}, err
	}
	body, err := fetchWith(ctx, "https://api.github.com/copilot_internal/user", tok)
	if err != nil {
		return user{}, err
	}
	defer body.Close()
	var u user
	err = jsonx.Decode(io.LimitReader(body, 16<<20), &u)
	return u, err
}

// apiBase is where your Copilot's agents API is, remembered once known.
func apiBase(ctx context.Context) (string, error) {
	baseMu.Lock()
	defer baseMu.Unlock()
	if baseURL != "" {
		return baseURL, nil
	}
	u, err := readUser(ctx)
	if err != nil {
		return "", err
	}
	if u.Endpoints.API == "" {
		return "", errors.New("copilot: GitHub gave no API for your Copilot")
	}
	baseURL = strings.TrimRight(u.Endpoints.API, "/")
	return baseURL, nil
}

var (
	baseMu  sync.Mutex
	baseURL string
)

// repoNames are repositories' full names by id, as GitHub gave them.
var repoNames sync.Map

// repoName is a repository's owner/name, by its id.
func repoName(ctx context.Context, id int64) string {
	if v, ok := repoNames.Load(id); ok {
		return v.(string)
	}
	var r struct {
		FullName string `json:"full_name"`
	}
	if get(ctx, fmt.Sprintf("https://api.github.com/repositories/%d", id), &r) != nil {
		return ""
	}
	repoNames.Store(id, r.FullName)
	return r.FullName
}
