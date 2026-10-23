// Package failure sorts the error a turn stopped on by what it says about
// the provider, and says it briefly without anything the error carried
// that could be a secret.
//
// Every harness words its errors its own way, so the sorting is by
// phrases: Claude Code's "API Error: 401 …", Codex's "unexpected status
// 429 Too Many Requests", an OAuth server's invalid_grant. An adapter whose
// agent says more adds its phrases with Add, at init.
package failure

import "strings"

// Kind is what an error says about the provider.
type Kind uint8

const (
	// Fatal is an error trying again doesn't get past and that says
	// nothing about the provider: the prompt too long, a bad request,
	// anything not known.
	Fatal Kind = iota
	// Limit is a usage limit: the account's plan has run out until it
	// resets.
	Limit
	// Auth is a login that no longer works: signed out, the token expired
	// or revoked, the key refused.
	Auth
	// Capacity is the provider busy or rate limiting: never the login, and
	// trying again later gets past it.
	Capacity
	// Offline is the network down, or the machine asleep: not the
	// provider.
	Offline
	// Transient is the provider failing for a moment: a dropped or stalled
	// connection, a 5xx, a timeout.
	Transient
)

func (k Kind) String() string {
	return [...]string{"fatal", "limit", "auth", "capacity", "offline", "transient"}[k]
}

// Retries is whether trying again gets past an error of this kind.
func (k Kind) Retries() bool { return k == Capacity || k == Offline || k == Transient }

// phrases are what each kind's errors say, lowercased, looked for in this
// order: a usage limit first, as its words ("limit reached") come with
// rate-limit ones too. Codes of three digits match only as a number of
// their own, so "4013 tokens" isn't a 401.
var phrases = []struct {
	kind Kind
	says []string
}{
	{Limit, []string{"usage limit", "limit reached", "usage_limit_reached"}},
	{Auth, []string{"401", "403", "invalid_grant", "invalid_token", "invalid_client", "invalid_api_key", "invalid api key",
		"invalid x-api-key", "unauthorized_client", "access_denied", "authentication", "permission_error", "unauthorized",
		"forbidden", "revoked", "oauth token", "failed to authenticate", "oauth session expired", "token has expired", "token expired", "expired token", "not logged in", "/login", "please log in", "log in again",
		"apikeyhelper"}},
	{Capacity, []string{"429", "529", "overloaded", "rate_limit", "rate limit", "too many requests", "insufficient_quota", "quota"}},
	{Offline, []string{"can't reach the api", "unable to connect to api", "no response from api", "went to sleep", "internet",
		"enotfound", "eai_again", "econnrefused", "enetunreach", "ehostunreach", "enetdown"}},
	{Transient, []string{"500", "502", "503", "504", "api error: 5", "internal server error", "server_error", "api_error",
		"temporarily", "service unavailable", "timed out", "timeout", "etimedout", "econnreset", "socket hang up",
		"connection error", "connection lost", "connection reset", "connection closed", "mid-response", "mid-stream",
		"stopped arriving"}},
}

// Add has errors that say any of says (any case) sort as k, for an
// adapter whose agent words its errors its own way. A single word is let
// into Summary too. Call it at init: the lists aren't guarded.
func Add(k Kind, says ...string) {
	for i := range phrases {
		if phrases[i].kind != k {
			continue
		}
		for _, s := range says {
			s = strings.ToLower(s)
			phrases[i].says = append(phrases[i].says, s)
			if !strings.ContainsFunc(s, notWord) {
				known[s] = true
			}
		}
	}
}

// KindOf is what err says about the provider.
func KindOf(err string) Kind {
	t := strings.ToLower(err)
	if strings.Contains(t, "hit your") && strings.Contains(t, "limit") {
		return Limit // "You've hit your session limit · resets 5am"
	}
	for _, p := range phrases {
		for _, s := range p.says {
			if contains(t, s) {
				return p.kind
			}
		}
	}
	return Fatal
}

// contains is whether t says s; a code of digits only as a number of its
// own.
func contains(t, s string) bool {
	if !isDigits(s) {
		return strings.Contains(t, s)
	}
	for from := 0; ; {
		i := strings.Index(t[from:], s)
		if i < 0 {
			return false
		}
		i += from
		end := i + len(s)
		if (i == 0 || !isAlnum(t[i-1])) && (end == len(t) || !isAlnum(t[end])) {
			return true
		}
		from = end
	}
}

// known are the only words a Summary may hold: error codes of providers
// and OAuth, network errors and a few plain words. A fixed list, so
// nothing from a token or key can ever pass, whatever its shape.
var known = map[string]bool{}

func init() {
	for _, w := range []string{
		"invalid_grant", "invalid_token", "invalid_client", "invalid_request", "invalid_api_key", "unauthorized_client",
		"access_denied", "authentication_error", "permission_error", "insufficient_quota", "rate_limit_error",
		"rate_limit_exceeded", "usage_limit_reached", "overloaded_error", "api_error", "server_error", "not_found_error",
		"model_not_found", "invalid_request_error", "context_length_exceeded", "request_too_large", "unauthorized",
		"forbidden", "expired", "revoked", "overloaded", "timeout", "aborted", "quota", "unavailable", "econnrefused",
		"econnreset", "enotfound", "etimedout", "eai_again",
	} {
		known[w] = true
	}
}

// Summary is err said briefly and safely: its HTTP status, when it gives
// one ("401", "status 429", "HTTP 503", "API Error: 529"), then the known
// words it holds, in order, once each, five at most. Nothing else of it is
// kept. With none of that, it is what its kind means in plain words.
func Summary(err string) string {
	var parts []string
	prev := ""
	for w := range strings.FieldsFuncSeq(err, notWord) {
		lw := strings.ToLower(w)
		switch {
		case len(parts) == 0 && isStatus(w) && (prev == "" || prev == "status" || prev == "http" || prev == "error"):
			parts = append(parts, w)
		case known[lw] && !hasPart(parts, lw):
			parts = append(parts, lw)
		}
		if len(parts) >= 5 {
			break
		}
		prev = lw
	}
	if len(parts) > 0 {
		return strings.Join(parts, " ")
	}
	switch KindOf(err) {
	case Limit:
		return "usage limit"
	case Auth:
		return "signed out"
	case Capacity:
		return "rate limited"
	case Offline:
		return "offline"
	}
	return "request failed"
}

func hasPart(parts []string, w string) bool {
	for _, p := range parts {
		if p == w {
			return true
		}
	}
	return false
}

// isStatus is an HTTP status a failure gives: 4xx or 5xx.
func isStatus(w string) bool { return len(w) == 3 && (w[0] == '4' || w[0] == '5') && isDigits(w) }

func isDigits(s string) bool {
	for i := range len(s) {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return s != ""
}

func isAlnum(c byte) bool {
	return c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z'
}

// notWord splits a Summary's words: letters, digits and underscores make
// them.
func notWord(r rune) bool {
	return (r < 'a' || r > 'z') && (r < 'A' || r > 'Z') && (r < '0' || r > '9') && r != '_'
}
