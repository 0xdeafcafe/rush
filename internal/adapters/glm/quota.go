package glm

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// The GLM Coding Plan's limits, as ZCode itself reads them: api.z.ai for
// Z.ai's keys, open.bigmodel.cn for BigModel's. Asking spends nothing.
var (
	hostIntl  = "https://api.z.ai"
	hostCN    = "https://open.bigmodel.cn"
	quotaPath = "/api/monitor/usage/quota/limit"
)

var errNoKey = errors.New("no GLM API key in ZCode: sign in to ZCode with a Coding Plan key")

// provider is one of ZCode's model providers in v2/config.json.
type provider struct {
	Enabled bool `json:"enabled"`
	Options struct {
		APIKey  string `json:"apiKey"`
		BaseURL string `json:"baseURL"`
	} `json:"options"`
}

// credentials is the key and base URL of the provider ZCode uses: the one
// ZCODE_PROVIDER pins, else the first enabled one with a key.
func credentials(dir string) (key, baseURL string) {
	b, err := os.ReadFile(filepath.Join(dir, "v2", "config.json"))
	if err != nil {
		return "", ""
	}
	var cfg struct {
		Provider map[string]provider `json:"provider"`
	}
	if jsonx.Unmarshal(b, &cfg) != nil {
		return "", ""
	}
	ids := make([]string, 0, len(cfg.Provider))
	for id := range cfg.Provider {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	pinned := os.Getenv("ZCODE_PROVIDER")
	for _, id := range ids {
		p := cfg.Provider[id]
		if pinned != "" && id != pinned {
			continue
		}
		if p.Enabled && p.Options.APIKey != "" {
			return p.Options.APIKey, p.Options.BaseURL
		}
	}
	return "", ""
}

// quotaResponse is the quota endpoint's envelope.
type quotaResponse struct {
	Code    int    `json:"code"`
	Msg     string `json:"msg"`
	Success bool   `json:"success"`
	Data    struct {
		Level  string  `json:"level"`
		Limits []limit `json:"limits"`
	} `json:"data"`
}

// limit is one of the plan's limits. Some count (usage is the allowance,
// currentValue what's used), some only say a percentage used.
type limit struct {
	Type          string   `json:"type"` // TOKENS_LIMIT, TIME_LIMIT (MCP calls)
	Unit          *int     `json:"unit"`
	Number        *int     `json:"number"`
	Usage         *float64 `json:"usage"`
	CurrentValue  *float64 `json:"currentValue"`
	Remaining     *float64 `json:"remaining"`
	Percentage    *float64 `json:"percentage"`
	NextResetTime *int64   `json:"nextResetTime"` // ms
}

// Quota is the Coding Plan's limits for the key ZCode is signed in with.
func (Adapter) Quota(ctx context.Context, p agent.Profile, _ agent.Account) (usage.Quota, error) {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	key, base := credentials(dir)
	if key == "" {
		return usage.Quota{}, errNoKey
	}
	host := hostCN
	if strings.Contains(base, "z.ai") {
		host = hostIntl
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, host+quotaPath, nil)
	if err != nil {
		return usage.Quota{}, err
	}
	req.Header.Set("Authorization", "Bearer "+key)
	req.Header.Set("Accept", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return usage.Quota{}, err
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	if res.StatusCode == http.StatusTooManyRequests {
		return usage.Quota{}, errors.New("GLM quota: rate-limited")
	}
	var r quotaResponse
	if err := jsonx.Unmarshal(body, &r); err != nil {
		return usage.Quota{}, fmt.Errorf("GLM quota: %s", res.Status)
	}
	if !r.Success {
		if r.Code == 401 || r.Code == 1001 {
			return usage.Quota{}, errors.New("GLM refused ZCode's API key")
		}
		return usage.Quota{}, fmt.Errorf("GLM quota: %s", r.Msg)
	}
	return quotaOf(r, time.Now()), nil
}

// quotaOf is the plan's limits as rush's windows.
func quotaOf(r quotaResponse, now time.Time) usage.Quota {
	q := usage.Quota{Plan: r.Data.Level, FetchedAt: now, Source: usage.Fetched}
	for _, l := range r.Data.Limits {
		w, ok := windowOf(l)
		if ok {
			q.Windows = append(q.Windows, w)
		}
	}
	return q
}

func windowOf(l limit) (usage.Window, bool) {
	var w usage.Window
	switch {
	case l.Type == "TOKENS_LIMIT" && l.Number != nil && *l.Number == 5:
		w = usage.Window{ID: "token_5h", Label: "5h", Name: "5-hour", Span: 5 * time.Hour}
	case l.Type == "TOKENS_LIMIT" && l.Number != nil && *l.Number == 7:
		w = usage.Window{ID: "token_week", Label: "7d", Name: "weekly", Span: 7 * 24 * time.Hour}
	case l.Type == "TOKENS_LIMIT":
		w = usage.Window{ID: "token", Label: "tokens", Name: "tokens"}
		if l.Number != nil {
			w.ID = fmt.Sprintf("token_%d", *l.Number)
		}
	case l.Type == "TIME_LIMIT" || l.Type == "MCP_LIMIT":
		w = usage.Window{ID: "mcp", Label: "MCP", Name: "MCP calls", Span: 30 * 24 * time.Hour}
	default:
		w = usage.Window{ID: strings.ToLower(l.Type), Label: l.Type, Name: l.Type}
	}
	// The most precise figure there is: what's left of the parts, what's
	// used of the allowance, then the percentage used.
	total := 0.0
	if l.Remaining != nil && l.CurrentValue != nil {
		total = *l.Remaining + *l.CurrentValue
	} else if l.Usage != nil {
		total = *l.Usage
	}
	switch {
	case total > 0 && l.CurrentValue != nil:
		w.Used, w.Limit = *l.CurrentValue, total
		w.Percent = 100 * *l.CurrentValue / total
	case total > 0 && l.Remaining != nil:
		w.Used, w.Limit = total-*l.Remaining, total
		w.Percent = 100 * (total - *l.Remaining) / total
	case l.Percentage != nil:
		w.Percent = *l.Percentage
	default:
		return w, false
	}
	w.Percent = max(0, min(100, w.Percent))
	if l.NextResetTime != nil && *l.NextResetTime > 0 {
		w.ResetsAt = time.UnixMilli(*l.NextResetTime)
	}
	return w, true
}
