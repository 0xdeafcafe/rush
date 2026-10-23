package host

import (
	"fmt"
	"slices"
	"strings"
	"unicode"

	"github.com/0xdeafcafe/rush/internal/agent"
)

// How spawn_agent finds the agent a session asks for. The caller needn't
// know the names spawn_agent lists: a model ("gpt-6.1-sol", "sol 6.1"), a
// harness or provider ("codex", "kimi") or both ("codex astra") find it
// too. See docs/subagents.md.

// find is the agent of ps that name, and model when given, ask for, and
// the model to run it on when it's other than the agent's own ("" for
// that one).
func find(ps []pick, name, model string) (pick, string, bool) {
	name, model = strings.TrimSpace(name), strings.TrimSpace(model)
	if model != "" {
		if p, ok := match(ps, name+" "+model); ok {
			return p, "", true
		}
	}
	p, ok := match(ps, or(name, model))
	if !ok || model == "" {
		return p, "", ok
	}
	return p, modelOf(&p.r, model), true
}

// match is the agent of ps that q names: one by its listed name, or by
// its model's id; else an agent, provider or harness by its own name, on
// its default model; else the one whose words hold all q's, the one with
// fewest words of its model left over first.
func match(ps []pick, q string) (pick, bool) {
	if q == "" {
		return pick{}, false
	}
	for i := range ps {
		if p := &ps[i]; p.name == q || p.name == cleanName(q) || p.model != "" && strings.EqualFold(p.model, q) {
			return *p, true
		}
	}
	for i := range ps {
		if slices.Contains(ownNames(&ps[i].r), cleanName(q)) {
			p := ps[i]
			p.name, p.model = q, ""
			return p, true
		}
	}
	qw := words(q)
	best, left, named := -1, 0, false
	for i := range ps {
		p := &ps[i]
		have := slices.Concat(words(p.name), words(p.model))
		for _, n := range ownNames(&p.r) {
			have = append(have, words(n)...)
		}
		if !holds(have, qw) {
			continue
		}
		mw := slices.Concat(words(p.model), words(p.name))
		n := 0
		for _, w := range words(p.model) {
			if !slices.Contains(qw, w) {
				n++
			}
		}
		if best < 0 || n < left {
			best, left = i, n
			named = slices.ContainsFunc(qw, func(w string) bool { return slices.Contains(mw, w) })
		}
	}
	if best < 0 {
		return pick{}, false
	}
	p := ps[best]
	if !named { // only its harness or provider was named: its default
		p.name, p.model = q, ""
	}
	return p, true
}

// modelOf is the model of r's that q names, by its id or its words; q
// itself when r lists none that fits, for its harness to take or refuse.
func modelOf(r *runner, q string) string {
	qw := words(q)
	best, left := "", 0
	for _, c := range r.models {
		if strings.EqualFold(c.ID, q) {
			return c.ID
		}
		mw := words(c.ID)
		if !holds(mw, qw) {
			continue
		}
		if n := len(mw) - len(qw); best == "" || n < left {
			best, left = c.ID, n
		}
	}
	return or(best, q)
}

// ownNames are what r is called apart from its models: its agent's,
// provider's and harness's names and labels, as cleanName writes them.
func ownNames(r *runner) []string {
	return []string{cleanName(string(r.k)), cleanName(r.prov), cleanName(string(agent.HarnessOf(r.k))),
		cleanName(agent.HarnessLabel(r.k)), cleanName(agent.ProviderLabel(r.prov))}
}

// holds is whether have has every word of want.
func holds(have, want []string) bool {
	if len(want) == 0 {
		return false
	}
	for _, w := range want {
		if !slices.Contains(have, w) {
			return false
		}
	}
	return true
}

// words are s's words, lower case: letters apart from numbers, and a
// version (6.1) kept whole, so "sol6.1" is sol and 6.1, as "gpt-6.1-sol"
// has them.
func words(s string) []string {
	var out []string
	var cur []rune
	digits := false
	flush := func() {
		if len(cur) > 0 {
			out = append(out, string(cur))
		}
		cur = cur[:0]
	}
	rs := []rune(strings.ToLower(s))
	for i, c := range rs {
		switch {
		case unicode.IsDigit(c):
			if len(cur) > 0 && !digits {
				flush()
			}
			digits = true
			cur = append(cur, c)
		case c == '.' && digits && len(cur) > 0 && i+1 < len(rs) && unicode.IsDigit(rs[i+1]):
			cur = append(cur, c)
		case unicode.IsLetter(c):
			if len(cur) > 0 && digits {
				flush()
			}
			digits = false
			cur = append(cur, c)
		default:
			flush()
		}
	}
	flush()
	return out
}

// noAgent is the error for an agent none of ps is, naming them all.
func noAgent(name, model string, ps []pick) error {
	q := strings.TrimSpace(name + " " + model)
	names, harnesses := make([]string, 0, len(ps)), []string{}
	for i := range ps {
		names = append(names, ps[i].name)
		if h := cleanName(string(agent.HarnessOf(ps[i].r.k))); !slices.Contains(harnesses, h) {
			harnesses = append(harnesses, h)
		}
	}
	if len(names) == 0 {
		return fmt.Errorf("rush has no agent matching %q, nor any other it can start here: Settings › Config switches harnesses and providers on", q)
	}
	return fmt.Errorf("rush has no agent matching %q. Name one of these: %s; or a harness (%s), a model, or both (\"codex astra\"). A harness switched off in Settings › Config isn't offered",
		q, strings.Join(names, ", "), strings.Join(harnesses, ", "))
}

// cleanName is s as an agent's name: lower case, a hyphen for anything
// but letters and numbers, none doubled or at the ends.
func cleanName(s string) string {
	s = strings.Trim(strings.Map(func(c rune) rune {
		if c >= 'a' && c <= 'z' || c >= '0' && c <= '9' {
			return c
		}
		return '-'
	}, strings.ToLower(s)), "-")
	for strings.Contains(s, "--") {
		s = strings.ReplaceAll(s, "--", "-")
	}
	return s
}
