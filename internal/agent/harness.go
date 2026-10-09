package agent

import (
	"slices"
	"strings"
)

// A provider is where a session's model comes from (Anthropic, OpenAI,
// Ollama on this machine); a harness is the program that runs the session
// around it (Claude Code, Codex, Pi). Most agents are both at once: Codex
// runs OpenAI's models. A Rider is a provider running in another agent's
// harness, and one provider can ride several: Ollama in Claude Code, in Pi
// and in Codex are each an adapter of their own, all of provider "ollama".

// Provided is a Rider that names the provider it runs, when that isn't its
// own kind: Ollama in Pi is kind "ollama-pi", provider "ollama".
type Provided interface {
	Provider() string
}

// Made is an agent that names the company behind the models it runs,
// when that isn't its own name: Claude Code's are Anthropic's.
type Made interface {
	Maker() string
}

// ProviderName is provider p by the company behind its models, else
// empty: the caller names it as the agent.
func ProviderName(p string) string {
	if a, ok := Get(Kind(p)); ok {
		if m, ok := a.(Made); ok {
			return m.Maker()
		}
	}
	return ""
}

// ProviderOf is the provider agent k runs: a Rider's Provider, else k.
func ProviderOf(k Kind) string {
	if a, ok := Get(k); ok {
		if p, ok := a.(Provided); ok {
			return p.Provider()
		}
	}
	return string(k)
}

// HarnessOf is the program agent k runs in: the agent a Rider rides, else
// k itself.
func HarnessOf(k Kind) Kind {
	if a, ok := Get(k); ok {
		if r, ok := a.(Rider); ok {
			return r.Rides()
		}
	}
	return k
}

// Providers are every provider an adapter runs, by name, each once.
func Providers() []string {
	var out []string
	for _, a := range All() {
		if CurrentKind(a.Kind()) != a.Kind() {
			continue
		}
		if p := ProviderOf(a.Kind()); !slices.Contains(out, p) {
			out = append(out, p)
		}
	}
	slices.Sort(out)
	return out
}

// IsProvider is whether p is one of Providers, without building them:
// a profile's name is looked up this way every frame.
func IsProvider(p string) bool {
	for _, a := range All() {
		if CurrentKind(a.Kind()) == a.Kind() && ProviderOf(a.Kind()) == p {
			return true
		}
	}
	return false
}

// Harnesses are the agents that run provider p, whether installed or not:
// the one of p's own kind first (its default), then the rest by kind.
func Harnesses(p string) []Kind {
	var out []Kind
	for _, a := range All() {
		if ProviderOf(a.Kind()) == p && CurrentKind(a.Kind()) == a.Kind() {
			out = append(out, a.Kind())
		}
	}
	slices.SortStableFunc(out, func(a, b Kind) int {
		switch {
		case string(a) == p:
			return -1
		case string(b) == p:
			return 1
		}
		return 0
	})
	return out
}

// KindFor is the agent that runs provider id p (as Billed reads it) in
// harness h: p's own kind when h is empty or p's own harness. When that one isn't installed, the
// first of p's that is. ok is false when p has no adapter at all.
func KindFor(p string, h Kind) (Kind, bool) {
	all := RunsFor(p)
	if len(all) == 0 {
		return "", false
	}
	if h != "" {
		for _, k := range all {
			if HarnessOf(k) == h && Runs(k) {
				return k, true
			}
		}
	}
	for _, k := range all {
		if Runs(k) {
			return k, true
		}
	}
	return all[0], true
}

// ProviderInstalled is whether any of provider p's agents runs here.
func ProviderInstalled(p string) bool {
	return slices.ContainsFunc(Harnesses(p), Runs)
}

// ProviderLabel is provider p as you'd call it: the company behind its
// models (Anthropic, OpenAI, GitHub), else its agent's name (Ollama).
func ProviderLabel(p string) string {
	if n := ProviderName(p); n != "" {
		return n
	}
	if a, ok := Get(Kind(p)); ok {
		return a.Name()
	}
	return p
}

// Label is agent k as rush names it everywhere: its provider, then the
// harness it runs in, OpenAI (Codex), Ollama (Pi); the one name alone
// when they're the same, OpenCode.
func Label(k Kind) string {
	p, h := ProviderLabel(ProviderOf(k)), HarnessLabel(k)
	if p == h || h == "" {
		return p
	}
	return p + " (" + h + ")"
}

// HarnessLabel is the program agent k runs in, by name: Claude Code for
// Ollama in Claude Code.
func HarnessLabel(k Kind) string {
	if a, ok := Get(HarnessOf(k)); ok {
		return a.Name()
	}
	return string(HarnessOf(k))
}

// Keyed is a provider that can be paid for per token with an API key,
// apart from any subscription: the variable its own programs read the
// key from.
type Keyed interface {
	KeyEnv() string
}

// KeyEnv is where provider p's programs read its API key from; "" when
// it takes none.
func KeyEnv(p string) string {
	if a, ok := Get(Kind(p)); ok {
		if k, ok := a.(Keyed); ok {
			return k.KeyEnv()
		}
	}
	return ""
}

// KeyOnly is whether agent k is paid for only with its provider's API
// key: a provider that takes one, in another's harness.
func KeyOnly(k Kind) bool {
	return HarnessOf(k) != k && KeyEnv(ProviderOf(k)) != "" && !onPlan(k)
}

// A provider paid for two ways is two providers in rush: its
// subscription, which only its own program signs in to, and its API key,
// which any harness that speaks its API can use. The key one is named
// KeyOf the provider: "claude-key" is Anthropic by API key.

// Split is whether provider p is paid for both ways: a subscription its
// own program signs in to, and an API key apart from it.
func Split(p string) bool {
	return KeyEnv(p) != "" && Supports(Kind(p), FeatureSignIn)
}

// KeyOf is the provider that is p paid for with its API key.
func KeyOf(p string) string { return p + keySuffix }

const keySuffix = "-key"

// Billed is the provider id names, and whether it's that one's API key:
// "claude-key" is claude by key, "claude" its subscription.
func Billed(id string) (p string, key bool) {
	if p, ok := strings.CutSuffix(id, keySuffix); ok && Split(p) {
		return p, true
	}
	return id, false
}

// RunsFor are the agents that run provider id: a split provider's
// subscription in its own program and in those that sign in to its plan
// (PlanRider), its key in any other that speaks its API; any other provider
// in all of Harnesses.
func RunsFor(id string) []Kind {
	p, key := Billed(id)
	all := Harnesses(p)
	if !Split(p) {
		return all
	}
	return slices.DeleteFunc(all, func(k Kind) bool {
		if key {
			return onPlan(k)
		}
		return k != Kind(p) && !onPlan(k)
	})
}
