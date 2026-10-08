package deepseek

import (
	"bufio"
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

// balanceURL is DeepSeek's balance endpoint. Asking it spends nothing.
var balanceURL = "https://api.deepseek.com/user/balance"

// errNoKey is a dsh with no DeepSeek API key rush can find.
var errNoKey = errors.New("no DeepSeek API key: set one with dsh, or DEEPSEEK_API_KEY")

// balance is /user/balance's answer.
type balance struct {
	IsAvailable  bool `json:"is_available"`
	BalanceInfos []struct {
		Currency        string `json:"currency"`
		TotalBalance    string `json:"total_balance"`
		GrantedBalance  string `json:"granted_balance"`
		ToppedUpBalance string `json:"topped_up_balance"`
	} `json:"balance_infos"`
}

// Quota is what's left on the DeepSeek API key dsh uses. DeepSeek bills
// as you go, so there are no windows, only the balance.
func (Adapter) Quota(ctx context.Context, p agent.Profile, _ agent.Account) (usage.Quota, error) {
	dir := p.Dir
	if dir == "" {
		dir = home()
	}
	key := apiKey(dir)
	if key == "" {
		return usage.Quota{}, errNoKey
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, balanceURL, nil)
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
	switch {
	case res.StatusCode == http.StatusUnauthorized:
		return usage.Quota{}, errors.New("DeepSeek refused the API key")
	case res.StatusCode != http.StatusOK:
		return usage.Quota{}, fmt.Errorf("DeepSeek balance: %s", res.Status)
	}
	var b balance
	if err := jsonx.Unmarshal(body, &b); err != nil {
		return usage.Quota{}, fmt.Errorf("DeepSeek balance: %w", err)
	}
	return quotaOf(b, time.Now()), nil
}

// quotaOf is a balance as rush's reading: "¥110.00 left", or why it can't
// be used.
func quotaOf(b balance, now time.Time) usage.Quota {
	q := usage.Quota{FetchedAt: now, Source: usage.Fetched}
	var parts []string
	for _, i := range b.BalanceInfos {
		parts = append(parts, money(i.Currency, i.TotalBalance))
	}
	if len(parts) > 0 {
		q.Balance = strings.Join(parts, " + ") + " left"
	}
	if !b.IsAvailable {
		q.Problem = "DeepSeek balance used up: top up to keep going"
	}
	return q
}

// money is an amount with its currency's sign.
func money(currency, amount string) string {
	if f, err := strconv.ParseFloat(amount, 64); err == nil {
		amount = strconv.FormatFloat(f, 'f', 2, 64)
	}
	switch strings.ToUpper(currency) {
	case "CNY":
		return "¥" + amount
	case "USD":
		return "$" + amount
	}
	return amount + " " + currency
}

// apiKey is the DeepSeek key dsh would use, in dsh's own order: the
// environment, its credential store ($DSH_HOME/.credentials.yaml), then
// $DSH_HOME/.env.
func apiKey(dir string) string {
	if k := os.Getenv("DEEPSEEK_API_KEY"); k != "" {
		return k
	}
	if k := credentialsKey(filepath.Join(dir, ".credentials.yaml")); k != "" {
		return k
	}
	return dotenvKey(filepath.Join(dir, ".env"))
}

// credentialsKey reads refs.DEEPSEEK_API_KEY from dsh's credential file:
//
//	version: 1
//	refs:
//	  DEEPSEEK_API_KEY: sk-…
//
// It reads just that, line by line, rather than all of YAML.
func credentialsKey(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()
	inRefs := false
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Text()
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if line[0] != ' ' && line[0] != '\t' {
			inRefs = strings.HasPrefix(trimmed, "refs:")
			continue
		}
		if inRefs {
			if v, ok := strings.CutPrefix(trimmed, "DEEPSEEK_API_KEY:"); ok {
				return unquote(v)
			}
		}
	}
	return ""
}

// dotenvKey reads DEEPSEEK_API_KEY from a .env file.
func dotenvKey(path string) string {
	b, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	for _, line := range strings.Split(string(b), "\n") {
		line = strings.TrimPrefix(strings.TrimSpace(line), "export ")
		if v, ok := strings.CutPrefix(line, "DEEPSEEK_API_KEY="); ok {
			return unquote(v)
		}
	}
	return ""
}

func unquote(v string) string {
	v = strings.TrimSpace(v)
	if v != "" && (v[0] == '"' || v[0] == '\'') {
		if end := strings.IndexByte(v[1:], v[0]); end >= 0 {
			return v[1 : 1+end]
		}
		return v
	}
	if i := strings.Index(v, " #"); i >= 0 {
		v = strings.TrimSpace(v[:i])
	}
	return v
}
