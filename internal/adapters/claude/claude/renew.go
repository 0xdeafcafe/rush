package claude

import (
	"bytes"
	"cmp"
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/proc"
)

// Claude Code's own refresh, as 2.1.284 has it (strings of its binary):
// ln().TOKEN_URL and ln().CLIENT_ID, a JSON POST of {grant_type:
// "refresh_token", refresh_token, client_id, scope}, answered with
// access_token, refresh_token (the old one when absent) and expires_in in
// seconds. A 400 or 401 saying invalid_grant is a revoked sign-in.
var tokenURL = "https://platform.claude.com/v1/oauth/token"

const clientID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"

var (
	ErrRevoked = errors.New("sign-in revoked · l signs in again")
	ErrInUse   = errors.New("in use by a running Claude Code, which refreshes it itself")
)

// Renew refreshes the sign-in a's folder holds, as Claude Code would there,
// unless a Claude Code runs there now: two refreshing one sign-in revoke
// each other's. It holds rush's vault lock and Claude Code's own refresh
// lock on the folder, and keeps the vault's copy in step.
func Renew(ctx context.Context, a Account) error {
	if InUse(a) {
		return ErrInUse
	}
	v := TheVault()
	unlock, err := v.Lock()
	if err != nil {
		return err
	}
	defer unlock()
	release, err := refreshLock(a.ConfigDir)
	if err != nil {
		return err
	}
	defer release()
	err = renew(ctx, func() ([]byte, error) { return readCreds(a) }, func(b []byte) error { return writeCreds(a, b) })
	if err == nil {
		_, _, _, _ = v.Keep(a)
	}
	return err
}

// RenewKept refreshes the sign-in the vault keeps for id, one no folder
// holds and so no Claude Code runs on.
func RenewKept(ctx context.Context, id string) error {
	v := TheVault()
	unlock, err := v.Lock()
	if err != nil {
		return err
	}
	defer unlock()
	return renew(ctx, func() ([]byte, error) { return v.Get(id) }, func(b []byte) error { return v.Put(id, b) })
}

// InUse is whether a Claude Code running now has a's folder as its
// config; one whose environment can't be read might, so it counts.
var InUse = func(a Account) bool {
	for _, p := range proc.Snapshot(nil).Procs {
		if filepath.Base(p.Comm) != "claude" {
			continue
		}
		env := proc.Env(p.PID)
		if env == nil || filepath.Clean(configDirIn(env)) == filepath.Clean(a.ConfigDir) {
			return true
		}
	}
	return false
}

// configDirIn is the folder a Claude Code with env runs in.
func configDirIn(env []string) string {
	for _, kv := range env {
		if v, ok := strings.CutPrefix(kv, "CLAUDE_CONFIG_DIR="); ok && v != "" {
			return v
		}
	}
	return DefaultAccount().ConfigDir
}

// refreshLock takes Claude Code's own lock on refreshing dir's sign-in:
// a directory, stale once a minute old. A Claude Code starting meanwhile
// waits for it, then finds the new sign-in rather than refreshing.
func refreshLock(dir string) (func(), error) {
	p := filepath.Join(dir, ".oauth_refresh.lock")
	if st, err := os.Stat(p); err == nil && time.Since(st.ModTime()) > time.Minute {
		_ = os.Remove(p)
	}
	if err := os.Mkdir(p, 0o700); err != nil {
		if os.IsExist(err) {
			return nil, ErrInUse
		}
		return nil, err
	}
	return func() { _ = os.Remove(p) }, nil
}

// oauthCreds is what a refresh reads of a sign-in.
type oauthCreds struct {
	OAuth struct {
		Refresh  string   `json:"refreshToken"`
		Scopes   []string `json:"scopes"`
		ClientID string   `json:"clientId"`
	} `json:"claudeAiOauth"`
}

// renew refreshes the sign-in read gives and writes it back, unless it
// changed meanwhile (another refresh's is as good).
func renew(ctx context.Context, read func() ([]byte, error), write func([]byte) error) error {
	cred, err := read()
	var c oauthCreds
	if err != nil || jsonx.Unmarshal(cred, &c) != nil || c.OAuth.Refresh == "" {
		return ErrNotSignedIn
	}
	next, err := refreshed(ctx, cred, c)
	now, rerr := read()
	var nc oauthCreds
	if rerr == nil && jsonx.Unmarshal(now, &nc) == nil && nc.OAuth.Refresh != c.OAuth.Refresh {
		return nil
	}
	if err != nil {
		return err
	}
	return write(next)
}

// refreshed is cred with the tokens its refresh token is exchanged for,
// every other field as it was. The scopes asked are the ones stored,
// which Claude Code falls back to when its wider ask is refused.
func refreshed(ctx context.Context, cred []byte, c oauthCreds) ([]byte, error) {
	body := map[string]string{"grant_type": "refresh_token", "refresh_token": c.OAuth.Refresh, "client_id": clientID}
	if c.OAuth.ClientID != "" {
		body["client_id"] = c.OAuth.ClientID
	}
	if len(c.OAuth.Scopes) > 0 {
		body["scope"] = strings.Join(c.OAuth.Scopes, " ")
	}
	b, err := jsonx.Marshal(body)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, "POST", tokenURL, bytes.NewReader(b))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 30 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	resp, err := client.Do(req)
	if err != nil {
		return nil, errors.New("refresh request failed")
	}
	defer resp.Body.Close()
	var got struct {
		Access    string         `json:"access_token"`
		Refresh   string         `json:"refresh_token"`
		ExpiresIn int64          `json:"expires_in"`
		Error     jsontext.Value `json:"error"`
	}
	_ = jsonx.Decode(io.LimitReader(resp.Body, 1<<20), &got)
	switch {
	case (resp.StatusCode == http.StatusBadRequest || resp.StatusCode == http.StatusUnauthorized) && oauthError(got.Error) == "invalid_grant":
		return nil, ErrRevoked
	case resp.StatusCode != http.StatusOK:
		return nil, fmt.Errorf("refresh returned %d", resp.StatusCode)
	case got.Access == "" || got.ExpiresIn <= 0:
		return nil, errors.New("refresh answered without a token")
	}
	return withTokens(cred, got.Access, cmp.Or(got.Refresh, c.OAuth.Refresh), time.Now().Add(time.Duration(got.ExpiresIn)*time.Second).UnixMilli())
}

// oauthError is an error answer's code: a string, or an object's type.
func oauthError(v jsontext.Value) string {
	var s string
	if jsonx.Unmarshal(v, &s) == nil {
		return s
	}
	var o struct {
		Type string `json:"type"`
	}
	_ = jsonx.Unmarshal(v, &o)
	return o.Type
}

// withTokens is cred with its access token, refresh token and expiry
// replaced, and nothing else changed.
func withTokens(cred []byte, access, refresh string, expiresAt int64) ([]byte, error) {
	var all, oauth map[string]jsontext.Value
	if err := jsonx.Unmarshal(cred, &all); err != nil {
		return nil, err
	}
	if err := jsonx.Unmarshal(all["claudeAiOauth"], &oauth); err != nil {
		return nil, err
	}
	for k, v := range map[string]any{"accessToken": access, "refreshToken": refresh, "expiresAt": expiresAt} {
		b, err := jsonx.Marshal(v)
		if err != nil {
			return nil, err
		}
		oauth[k] = b
	}
	b, err := jsonx.Marshal(oauth)
	if err != nil {
		return nil, err
	}
	all["claudeAiOauth"] = b
	return jsonx.Marshal(all)
}

// renewedAt is when each sign-in was last refreshed on its own, by key.
var renewedAt sync.Map

// orRenew is fetch's reading; when its sign-in has expired it's refreshed
// first, at most once in ten minutes for key. Only a revoked sign-in
// shows as the refresh's own error.
func orRenew(key string, fetch func() (Usage, error), renew func() error) (Usage, error) {
	u, err := fetch()
	if !errors.Is(err, ErrExpired) {
		return u, err
	}
	if t, ok := renewedAt.Load(key); ok && time.Since(t.(time.Time)) < 10*time.Minute {
		return u, err
	}
	renewedAt.Store(key, time.Now())
	switch rerr := renew(); {
	case rerr == nil:
		return fetch()
	case errors.Is(rerr, ErrRevoked):
		return u, rerr
	}
	return u, err
}

// RenewKeptOnExpiry is fetch, the vault's sign-in for id refreshed first
// when it has expired: see orRenew.
func RenewKeptOnExpiry(ctx context.Context, id string, fetch func() (Usage, error)) (Usage, error) {
	return orRenew(Login{ID: id}.UsageKey(), fetch, func() error { return RenewKept(ctx, id) })
}
