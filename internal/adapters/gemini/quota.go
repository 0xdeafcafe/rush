package gemini

import (
	"bytes"
	"cmp"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Code Assist is what Gemini signed in with Google runs on, and where it
// reads its own limits: loadCodeAssist for the tier and project,
// retrieveUserQuota for what's left of each model's allowance. Asking
// spends nothing.
var endpoint = "https://cloudcode-pa.googleapis.com/v1internal"

// credsFile is where Gemini keeps its Google sign-in, in its home.
const credsFile = "oauth_creds.json"

var (
	errNoGoogle = errors.New("not signed in with Google: an API key has no allowance to read")
	// Gemini renews its token only as it runs; rush doesn't sign in for it.
	errLapsed = errors.New("Gemini's Google sign-in has lapsed: it renews when gemini next runs")
)

// freeTier is Code Assist's tier for a Google account that pays nothing.
const freeTier = "free-tier"

type tier struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type loadResponse struct {
	CurrentTier *tier  `json:"currentTier"`
	PaidTier    *tier  `json:"paidTier"`
	Project     string `json:"cloudaicompanionProject"`
}

// bucket is one model's allowance: what fraction of it is left, and when
// it fills again.
type bucket struct {
	RemainingAmount   string   `json:"remainingAmount"`
	RemainingFraction *float64 `json:"remainingFraction"`
	ResetTime         string   `json:"resetTime"`
	TokenType         string   `json:"tokenType"`
	ModelID           string   `json:"modelId"`
}

type quotaResponse struct {
	Buckets []bucket `json:"buckets"`
}

// Quota is the Google account's allowance, read with the token Gemini
// keeps; an API key has none.
func (Adapter) Quota(ctx context.Context, p agent.Profile, _ agent.Account) (usage.Quota, error) {
	dir := p.Dir
	if dir == "" {
		dir = geminiHome()
	}
	var c struct {
		AccessToken string `json:"access_token"`
		Expiry      int64  `json:"expiry_date"` // ms
	}
	b, err := os.ReadFile(filepath.Join(dir, credsFile))
	if err != nil || jsonx.Unmarshal(b, &c) != nil || c.AccessToken == "" {
		return usage.Quota{}, errNoGoogle
	}
	if c.Expiry > 0 && time.Now().After(time.UnixMilli(c.Expiry)) {
		return usage.Quota{}, errLapsed
	}
	var load loadResponse
	proj := os.Getenv("GOOGLE_CLOUD_PROJECT")
	meta := map[string]any{"metadata": map[string]string{"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}
	if proj != "" {
		meta["cloudaicompanionProject"] = proj
	}
	if err := post(ctx, c.AccessToken, "loadCodeAssist", meta, &load); err != nil {
		return usage.Quota{}, err
	}
	proj = cmp.Or(load.Project, proj)
	if proj == "" {
		return usage.Quota{}, errors.New("Gemini hasn't set Code Assist up for this account: run gemini once")
	}
	var r quotaResponse
	if err := post(ctx, c.AccessToken, "retrieveUserQuota", map[string]string{"project": proj}, &r); err != nil {
		return usage.Quota{}, err
	}
	q := quotaOf(load, r, time.Now())
	q.Email = activeEmail(dir)
	return q, nil
}

// post calls a Code Assist method.
func post(ctx context.Context, token, method string, body, out any) error {
	b, err := jsonx.Marshal(body)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint+":"+method, bytes.NewReader(b))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	data, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	switch {
	case res.StatusCode == http.StatusUnauthorized:
		return errLapsed
	case res.StatusCode == http.StatusTooManyRequests:
		return errors.New("Gemini's limits: rate-limited")
	case res.StatusCode >= 300:
		return fmt.Errorf("Gemini's limits: %s", res.Status)
	}
	return jsonx.Unmarshal(data, out)
}

// quotaOf is the allowance as rush's windows, one a model: a day's
// requests, reset each day. The free tier's plan says so.
func quotaOf(load loadResponse, r quotaResponse, now time.Time) usage.Quota {
	q := usage.Quota{FetchedAt: now, Source: usage.Fetched}
	switch t := cmp.Or(load.PaidTier, load.CurrentTier); {
	case t == nil:
	case t.ID == freeTier:
		q.Plan = "free"
	default:
		q.Plan = cmp.Or(t.Name, t.ID)
	}
	for _, b := range r.Buckets {
		if b.ModelID == "" || b.RemainingFraction == nil {
			continue
		}
		left := max(0, min(1, *b.RemainingFraction))
		w := usage.Window{ID: b.ModelID, Label: strings.TrimPrefix(b.ModelID, "gemini-"), Name: b.ModelID + " today",
			Span: 24 * time.Hour, Percent: 100 * (1 - left), Scope: usage.Scope{Models: []string{b.ModelID}}}
		if n, err := strconv.ParseFloat(b.RemainingAmount, 64); err == nil && left > 0 {
			w.Limit = float64(int(n/left + 0.5))
			w.Used = w.Limit - n
		}
		if t, err := time.Parse(time.RFC3339, b.ResetTime); err == nil {
			w.ResetsAt = t
		}
		q.Windows = append(q.Windows, w)
	}
	return q
}

// activeEmail is the Google account Gemini is signed in as.
func activeEmail(dir string) string {
	var a struct {
		Active string `json:"active"`
	}
	b, _ := os.ReadFile(filepath.Join(dir, "google_accounts.json"))
	_ = jsonx.Unmarshal(b, &a)
	return a.Active
}
