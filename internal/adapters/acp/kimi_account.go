package acp

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/photon/keychain"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/pelletier/go-toml/v2"
)

// kimiConfig is what rush reads of Kimi's config.toml (or its older
// config.json).
type kimiConfig struct {
	Default string `toml:"default_model" json:"default_model"`
	Models  map[string]struct {
		Provider string `toml:"provider" json:"provider"`
		Model    string `toml:"model" json:"model"`
		Display  string `toml:"display_name" json:"display_name"`
		Context  int64  `toml:"max_context_size" json:"max_context_size"`
	} `toml:"models" json:"models"`
	Providers map[string]struct {
		Type    string `toml:"type" json:"type"`
		BaseURL string `toml:"base_url" json:"base_url"`
		APIKey  string `toml:"api_key" json:"api_key"`
		OAuth   *struct {
			Key     string `toml:"key" json:"key"`
			Storage string `toml:"storage" json:"storage"`
		} `toml:"oauth" json:"oauth"`
	} `toml:"providers" json:"providers"`
}

func readKimiConfig(dir string) (kimiConfig, error) {
	var cfg kimiConfig
	b, err := os.ReadFile(filepath.Join(dir, "config.toml"))
	if err == nil {
		err = toml.Unmarshal(b, &cfg)
		return cfg, err
	}
	if !os.IsNotExist(err) {
		return cfg, err
	}
	if b, err = os.ReadFile(filepath.Join(dir, "config.json")); err != nil {
		return cfg, err
	}
	err = jsonx.Unmarshal(b, &cfg)
	return cfg, err
}

// kimiCred is the credential of the provider Kimi runs on: its default
// model's, else the first model's that has one. token is what goes in the
// Authorization header; it is never shown.
type kimiCred struct {
	provider, method, baseURL, token string
	refreshable                      bool
	expired                          bool // OAuth access token past its expiry; Kimi refreshes it on next run
}

func kimiCredential(dir string) kimiCred {
	cfg, err := readKimiConfig(dir)
	if err != nil {
		return kimiCred{}
	}
	names := make([]string, 0, len(cfg.Models))
	for name := range cfg.Models {
		names = append(names, name)
	}
	sort.Strings(names)
	if _, ok := cfg.Models[cfg.Default]; ok {
		names = []string{cfg.Default} // another model's account is not the active account
	}
	for _, name := range names {
		key := cfg.Models[name].Provider
		prov, ok := cfg.Providers[key]
		if !ok {
			continue
		}
		c := kimiCred{provider: key, baseURL: prov.BaseURL}
		if prov.APIKey != "" {
			c.method, c.token = "API key · Kimi config", prov.APIKey
			return c
		}
		ref := prov.OAuth
		if ref == nil || ref.Key == "" {
			continue
		}
		var tok struct {
			Access  string  `json:"access_token"`
			Refresh string  `json:"refresh_token"`
			Expires float64 `json:"expires_at"`
		}
		b, err := os.ReadFile(filepath.Join(dir, "credentials", filepath.Base(ref.Key)+".json"))
		c.method = "OAuth · Kimi credentials file"
		if err != nil && ref.Storage == "keyring" {
			b, err = keychain.Read("kimi-code", ref.Key)
			c.method = "OAuth · system keychain"
		}
		if err != nil || jsonx.Unmarshal(b, &tok) != nil || tok.Access == "" && tok.Refresh == "" {
			continue
		}
		c.refreshable = tok.Refresh != ""
		c.token = tok.Access
		c.expired = tok.Access == "" || tok.Expires > 0 && time.Unix(int64(tok.Expires), 0).Before(time.Now())
		if c.token == "" {
			c.token = "refresh-only" // signed in; Kimi mints an access token when it next runs
		}
		return c
	}
	return kimiCred{}
}

// kimiPlatforms names the endpoints Kimi's managed providers use.
var kimiPlatforms = map[string]string{
	"managed:kimi-code":   "Kimi Code",
	"managed:moonshot-cn": "Moonshot AI (moonshot.cn)",
	"managed:moonshot-ai": "Moonshot AI (moonshot.ai)",
}

// AccountSummary is the provider Kimi runs on and how it signs in. Kimi
// keeps no name or email on disk; the plan's limits are its Quota.
func (Kimi) AccountSummary(p agent.Profile) agent.AccountSummary {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	c := kimiCredential(p.Dir)
	if c.token == "" {
		return agent.AccountSummary{Status: "unknown", Note: "Rush could not verify Kimi credentials in this config home. Kimi may use credentials that Rush cannot read; this does not establish that you are signed out."}
	}
	s := agent.AccountSummary{SignedIn: true, Status: "credentials available", Method: c.method, Plan: kimiPlatforms[c.provider]}
	if s.Plan == "" {
		s.Plan = c.provider
	}
	if u, err := url.Parse(c.baseURL); err == nil && u.Host != "" {
		s.Endpoint = u.Scheme + "://" + u.Host
	}
	s.Note = "Kimi doesn't keep the account's name or email locally."
	if c.expired {
		if c.refreshable {
			s.Note = "Access token expired; a refresh credential is available for Kimi to renew it."
		} else {
			s.SignedIn = false
			s.Status = "access token expired"
			s.Note = "Stored access token expired and no refresh credential was found. Check sign-in in Kimi."
		}
	}
	return s
}

// kimiUsageURL is where Kimi Code says what's left of the plan: only Kimi
// Code has one.
func kimiUsageURL(c kimiCred) (string, bool) {
	if c.provider != "managed:kimi-code" {
		return "", false
	}
	base := os.Getenv("KIMI_CODE_BASE_URL")
	if base == "" {
		base = c.baseURL
	}
	if base == "" {
		base = "https://api.kimi.com/coding/v1"
	}
	return strings.TrimRight(base, "/") + "/usages", true
}

// Quota is the Kimi Code plan's limits, as Kimi's own /usage reads them.
// Asking spends nothing.
func (Kimi) Quota(ctx context.Context, p agent.Profile, _ agent.Account) (usage.Quota, error) {
	if p.Dir == "" {
		p.Dir = kimiHome()
	}
	c := kimiCredential(p.Dir)
	if c.token == "" {
		return usage.Quota{}, errors.New("Kimi has no credentials: run kimi login")
	}
	u, ok := kimiUsageURL(c)
	if !ok {
		return usage.Quota{}, errors.New("Kimi reports limits only for Kimi Code plans")
	}
	if c.expired {
		return usage.Quota{}, errors.New("Kimi's access token expired: run kimi once to refresh it")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return usage.Quota{}, err
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	req.Header.Set("Accept", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return usage.Quota{}, err
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	switch {
	case res.StatusCode == http.StatusUnauthorized:
		return usage.Quota{}, errors.New("Kimi refused its credentials: run kimi login")
	case res.StatusCode != http.StatusOK:
		return usage.Quota{}, fmt.Errorf("Kimi usage: %s", res.Status)
	}
	return kimiQuotaOf(body, time.Now())
}

// kimiNum is a number Kimi may send as a string.
type kimiNum struct {
	v  float64
	ok bool
}

func (n *kimiNum) UnmarshalJSON(b []byte) error {
	s := strings.Trim(string(b), `"`)
	v, err := strconv.ParseFloat(s, 64)
	n.v, n.ok = v, err == nil
	return nil
}

type kimiLimit struct {
	Name, Title, Scope string
	Limit, Used        kimiNum
	Remaining          kimiNum
	Duration           kimiNum
	TimeUnit           string  `json:"timeUnit"`
	ResetAt            string  `json:"reset_at"`
	ResetAt2           string  `json:"resetAt"`
	ResetTime          string  `json:"reset_time"`
	ResetTime2         string  `json:"resetTime"`
	ResetIn            kimiNum `json:"reset_in"`
	ResetIn2           kimiNum `json:"resetIn"`
}

func kimiQuotaOf(body []byte, now time.Time) (usage.Quota, error) {
	var r struct {
		User struct {
			Membership struct{ Level string } `json:"membership"`
		} `json:"user"`
		Usage  *kimiLimit `json:"usage"`
		Limits []struct {
			kimiLimit
			Detail *kimiLimit `json:"detail"`
			Window *kimiLimit `json:"window"`
		} `json:"limits"`
	}
	if err := jsonx.Unmarshal(body, &r); err != nil {
		return usage.Quota{}, fmt.Errorf("Kimi usage: %w", err)
	}
	q := usage.Quota{Plan: "Kimi Code", FetchedAt: now, Source: usage.Fetched}
	if lvl := strings.TrimPrefix(r.User.Membership.Level, "LEVEL_"); lvl != "" {
		q.Plan += " · " + strings.ToUpper(lvl[:1]) + strings.ToLower(lvl[1:])
	}
	if r.Usage != nil {
		if w, ok := kimiWindow(*r.Usage, kimiLimit{}, now); ok {
			if w.Name == "" {
				w.ID, w.Label, w.Name, w.Span = "weekly", "7d", "weekly", 7*24*time.Hour
			}
			q.Windows = append(q.Windows, w)
		}
	}
	for i, l := range r.Limits {
		d, win := l.kimiLimit, kimiLimit{}
		if l.Detail != nil {
			d = *l.Detail
		}
		if l.Window != nil {
			win = *l.Window
		}
		if win.Duration == (kimiNum{}) {
			win.Duration, win.TimeUnit = l.Duration, l.TimeUnit
		}
		if d.Name == "" && d.Title == "" && d.Scope == "" {
			d.Name = cmp.Or(l.Name, l.Title, l.Scope)
		}
		if w, ok := kimiWindow(d, win, now); ok {
			if w.Name == "" {
				w.ID, w.Label, w.Name = fmt.Sprintf("limit_%d", i+1), fmt.Sprintf("#%d", i+1), fmt.Sprintf("limit %d", i+1)
			}
			q.Windows = append(q.Windows, w)
		}
	}
	return q, nil
}

// kimiWindow is one of Kimi's limits as a rush window; win carries its
// duration when Kimi sends one.
func kimiWindow(d, win kimiLimit, now time.Time) (usage.Window, bool) {
	if !d.Limit.ok || d.Limit.v <= 0 {
		return usage.Window{}, false
	}
	used := d.Used.v
	if !d.Used.ok {
		if !d.Remaining.ok {
			return usage.Window{}, false
		}
		used = d.Limit.v - d.Remaining.v
	}
	w := usage.Window{Used: used, Limit: d.Limit.v, Percent: max(0, min(100, 100*used/d.Limit.v))}
	if win.Duration.ok && win.Duration.v > 0 {
		n := time.Duration(win.Duration.v)
		switch u := strings.ToUpper(win.TimeUnit); {
		case strings.Contains(u, "MINUTE"):
			w.Span = n * time.Minute
		case strings.Contains(u, "HOUR"):
			w.Span = n * time.Hour
		case strings.Contains(u, "DAY"):
			w.Span = n * 24 * time.Hour
		default:
			w.Span = n * time.Second
		}
		w.Label = kimiSpan(w.Span)
		w.ID, w.Name = "window_"+w.Label, w.Label
	}
	if name := cmp.Or(d.Name, d.Title, d.Scope); name != "" {
		w.Name = name
		if w.Label == "" {
			w.Label, w.ID = name, strings.ToLower(strings.ReplaceAll(name, " ", "_"))
		}
	}
	for _, s := range []string{d.ResetAt, d.ResetAt2, d.ResetTime, d.ResetTime2} {
		if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
			w.ResetsAt = t
			break
		}
	}
	if w.ResetsAt.IsZero() {
		for _, n := range []kimiNum{d.ResetIn, d.ResetIn2} {
			if n.ok && n.v > 0 {
				w.ResetsAt = now.Add(time.Duration(n.v) * time.Second)
				break
			}
		}
	}
	return w, true
}

func kimiSpan(d time.Duration) string {
	switch {
	case d%(24*time.Hour) == 0:
		return fmt.Sprintf("%dd", d/(24*time.Hour))
	case d%time.Hour == 0:
		return fmt.Sprintf("%dh", d/time.Hour)
	default:
		return fmt.Sprintf("%dm", d/time.Minute)
	}
}

var (
	_ agent.QuotaSource          = Kimi{}
	_ agent.AccountSummaryReader = Kimi{}
)
