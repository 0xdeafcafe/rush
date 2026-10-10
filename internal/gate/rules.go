package gate

import (
	"strconv"
	"strings"
	"time"
)

// A Rule is how one program is gated.
type Rule struct {
	Scope    string // worktree, repo or system
	Parallel int
	Stagger  time.Duration
	// Busy is the load per core above which a new run waits while another
	// in its scope runs; 0 is never.
	Busy float64
}

// Scopes are the scopes a rule may name.
var Scopes = []string{"worktree", "repo", "system"}

// Defaults are the settings' own, for any left unset.
var Defaults = map[string]string{
	"names": "tsc, tsgo, go, cargo, rustc, webpack, vite, next, vitest, jest, playwright, " +
		"golangci-lint, oxlint, eslint, biome",
	"scope":    "system",
	"parallel": "3",
	"stagger":  "3s",
	"busy":     "1",
}

// ParseRules is the rule for each program from the gate's settings: names
// takes scope, parallel and stagger, and overrides such as
// "tsc=system/1/10s, go=worktree/4" change any part for one name, or add
// it. Parts it can't read are left as they were; a setting missing takes
// its default.
func ParseRules(v map[string]string) map[string]Rule {
	get := func(k string) string {
		if s, ok := v[k]; ok {
			return strings.TrimSpace(s)
		}
		return Defaults[k]
	}
	base := Rule{Scope: "system", Parallel: 3, Stagger: 3 * time.Second}
	base = withPart(withPart(withPart(base, get("scope")), get("parallel")), get("stagger"))
	if b, err := strconv.ParseFloat(get("busy"), 64); err == nil && b >= 0 {
		base.Busy = b
	}
	out := map[string]Rule{}
	for _, n := range fields(get("names")) {
		out[n] = base
	}
	for _, o := range fields(v["overrides"]) {
		name, parts, _ := strings.Cut(o, "=")
		if name = strings.TrimSpace(name); name == "" {
			continue
		}
		r, ok := out[name]
		if !ok {
			r = base
		}
		for p := range strings.SplitSeq(parts, "/") {
			r = withPart(r, strings.TrimSpace(p))
		}
		out[name] = r
	}
	return out
}

// withPart is r with p read as whichever part it looks like: a scope, a
// count or a duration.
func withPart(r Rule, p string) Rule {
	if n, err := strconv.Atoi(p); err == nil && n > 0 {
		r.Parallel = n
	} else if d, err := time.ParseDuration(p); err == nil && d >= 0 {
		r.Stagger = d
	} else {
		for _, s := range Scopes {
			if p == s {
				r.Scope = s
			}
		}
	}
	return r
}

func fields(s string) []string {
	return strings.FieldsFunc(s, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' || r == '\n' })
}
