package claude

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// ErrRateLimited carries when the usage endpoint says to try again.
type ErrRateLimited struct{ Until time.Time }

func (e *ErrRateLimited) Error() string { return "usage refresh rate-limited" }

var (
	ErrNotSignedIn = errors.New("not signed in")
	ErrExpired     = errors.New(usage.Expired)
)

// keychainService is where Claude Code keeps an account's sign-in: the
// default account unsuffixed, any other suffixed with its folder's hash.
func (a Account) keychainService() string {
	if a.IsDefault() {
		return "Claude Code-credentials"
	}
	sum := sha256.Sum256([]byte(a.ConfigDir))
	return "Claude Code-credentials-" + hex.EncodeToString(sum[:])[:8]
}

// FetchUsage asks Anthropic for an account's plan usage with the account's
// own sign-in. The token is read for this request only and never kept.
func FetchUsage(ctx context.Context, a Account) (Usage, error) {
	who := SignedInAs(a)
	raw, err := readCreds(a)
	if err != nil {
		return Usage{}, ErrNotSignedIn
	}
	// Whose reading this is: ~/.claude may be signed in as another account
	// by the time it's looked at, and the sign-in may not be the account
	// the folder names. Unless Anthropic says whose, it's no one's.
	id, err := Owner(ctx, raw)
	if err != nil {
		return Usage{AccountID: who}, fmt.Errorf("couldn't tell whose sign-in it is: %w", err)
	}
	u, err := FetchUsageWith(ctx, raw)
	u.AccountID = id
	return u, err
}

// FetchUsageAs is FetchUsageWith for a sign-in kept as account id's: it's
// asked only once Anthropic says the sign-in is id's, so a sign-in kept
// under the wrong name never gives its reading to another account.
func FetchUsageAs(ctx context.Context, raw []byte, id string) (Usage, error) {
	owner, err := Owner(ctx, raw)
	switch {
	case err != nil:
		return Usage{}, fmt.Errorf("couldn't tell whose sign-in it is: %w", err)
	case owner != id:
		return Usage{}, errors.New("rush's sign-in for it is another account's; sign in again")
	}
	u, err := FetchUsageWith(ctx, raw)
	u.AccountID = id
	return u, err
}

// FetchUsageWith is FetchUsage with a sign-in already in hand: a login's
// that isn't the one in use.
func FetchUsageWith(ctx context.Context, raw []byte) (Usage, error) {
	token, err := accessToken(raw)
	if err != nil {
		return Usage{}, err
	}
	resp, err := oauthGet(ctx, "usage", token)
	if err != nil {
		return Usage{}, errors.New("usage request failed")
	}
	defer resp.Body.Close()
	switch resp.StatusCode {
	case http.StatusOK:
	case http.StatusTooManyRequests:
		return Usage{}, &ErrRateLimited{Until: retryAfter(resp.Header.Get("Retry-After"))}
	case http.StatusUnauthorized:
		return Usage{}, ErrExpired
	default:
		return Usage{}, fmt.Errorf("usage request returned %d", resp.StatusCode)
	}
	var data struct {
		FiveHour *rawWindow `json:"five_hour"`
		SevenDay *rawWindow `json:"seven_day"`
	}
	if err := jsonx.Decode(io.LimitReader(resp.Body, 1<<20), &data); err != nil {
		return Usage{}, err
	}
	return Usage{FiveHour: data.FiveHour.window(), SevenDay: data.SevenDay.window(), FetchedAt: time.Now(), Fetched: true}, nil
}

// accessToken is a sign-in's token, while it still works.
func accessToken(raw []byte) (string, error) {
	var cred struct {
		OAuth struct {
			Token     string `json:"accessToken"`
			ExpiresAt int64  `json:"expiresAt"`
		} `json:"claudeAiOauth"`
	}
	if jsonx.Unmarshal(raw, &cred) != nil || cred.OAuth.Token == "" {
		return "", ErrNotSignedIn
	}
	if cred.OAuth.ExpiresAt > 0 && time.UnixMilli(cred.OAuth.ExpiresAt).Before(time.Now()) {
		return "", ErrExpired
	}
	return cred.OAuth.Token, nil
}

func oauthGet(ctx context.Context, what, token string) (*http.Response, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", "https://api.anthropic.com/api/oauth/"+what, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("anthropic-beta", "oauth-2025-04-20")
	client := &http.Client{Timeout: 15 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	return client.Do(req)
}

// owners remembers whose each token is, by its hash: a token only ever
// belongs to one account, so each is asked about once.
var owners sync.Map

// Owner is the uuid of the account a sign-in belongs to, asked of
// Anthropic with the sign-in itself. Which account a config folder says
// it's signed in as can be wrong: a Claude Code started before a switch
// writes the old account's sign-in back when it refreshes it, leaving the
// new one's name beside it.
func Owner(ctx context.Context, raw []byte) (string, error) {
	token, err := accessToken(raw)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256([]byte(token))
	if id, ok := owners.Load(sum); ok {
		return id.(string), nil
	}
	resp, err := oauthGet(ctx, "profile", token)
	if err != nil {
		return "", errors.New("profile request failed")
	}
	defer resp.Body.Close()
	switch resp.StatusCode {
	case http.StatusOK:
	case http.StatusUnauthorized:
		return "", ErrExpired
	default:
		return "", fmt.Errorf("profile request returned %d", resp.StatusCode)
	}
	var p struct {
		Account struct {
			UUID string `json:"uuid"`
		} `json:"account"`
	}
	if err := jsonx.Decode(io.LimitReader(resp.Body, 1<<20), &p); err != nil {
		return "", err
	}
	if p.Account.UUID == "" {
		return "", errors.New("profile names no account")
	}
	owners.Store(sum, p.Account.UUID)
	return p.Account.UUID, nil
}

func retryAfter(v string) time.Time {
	min := time.Now().Add(15 * time.Minute)
	if s, err := strconv.Atoi(v); err == nil {
		if t := time.Now().Add(time.Duration(s) * time.Second); t.After(min) {
			return t
		}
	}
	return min
}
