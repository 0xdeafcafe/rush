package gate

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// A Rule is how one program is gated: the memory a run of it is expected
// to need before it may start, and how long it may run.
type Rule struct {
	Memory uint64 // bytes
	Limit  time.Duration
}

// Cap is the most memory a run's processes may hold together before the
// watchdog stops it: three times what it was expected to need.
func (r Rule) Cap() uint64 { return 3 * r.Memory }

const gib = 1 << 30

// Estimates are the memory a run of each program is expected to need;
// a program named without one expects otherEstimate.
var Estimates = map[string]uint64{
	"tsc": 4 * gib, "tsgo": 4 * gib, "tsrs": 2 * gib,
	"vitest": 3 * gib, "jest": 3 * gib,
	"playwright": 2 * gib,
	"next":       3 * gib, "webpack": 3 * gib, "vite": 3 * gib,
	"go": 2 * gib, "cargo": 4 * gib, "rustc": 2 * gib,
	"golangci-lint": 1 * gib, "oxlint": 1 * gib, "eslint": 1 * gib, "biome": 1 * gib,
}

const otherEstimate = 2 * gib

// Defaults are the settings' own, for any left unset.
var Defaults = map[string]string{
	"names": "tsc, tsgo, tsrs, go, cargo, rustc, webpack, vite, next, vitest, jest, playwright, " +
		"golangci-lint, oxlint, eslint, biome",
	"limit":   "30m",
	"reserve": "15%",
	"floor":   "5%",
}

// ParseRules is the rule for each program from the gate's settings: names
// says which, limit how long any may run, and limits such as
// "tsc=6G/45m, vitest=4G" change a program's memory or time, or add it.
// Parts it can't read are left as they were; a setting missing takes its
// default. The slot pool's settings (scope, parallel, stagger, busy,
// overrides) are read by no one now.
func ParseRules(v map[string]string) map[string]Rule {
	get := func(k string) string {
		if s, ok := v[k]; ok {
			return strings.TrimSpace(s)
		}
		return Defaults[k]
	}
	limit := 30 * time.Minute
	if d, err := time.ParseDuration(get("limit")); err == nil && d > 0 {
		limit = d
	}
	rule := func(name string) Rule {
		m, ok := Estimates[name]
		if !ok {
			m = otherEstimate
		}
		return Rule{Memory: m, Limit: limit}
	}
	out := map[string]Rule{}
	for _, n := range fields(get("names")) {
		out[n] = rule(n)
	}
	for _, o := range fields(v["limits"]) {
		name, parts, _ := strings.Cut(o, "=")
		if name = strings.TrimSpace(name); name == "" {
			continue
		}
		r, ok := out[name]
		if !ok {
			r = rule(name)
		}
		for p := range strings.SplitSeq(parts, "/") {
			p = strings.TrimSpace(p)
			if d, err := time.ParseDuration(p); err == nil && d > 0 {
				r.Limit = d
			} else if b, ok := ParseSize(p); ok {
				r.Memory = b
			}
		}
		out[name] = r
	}
	return out
}

// ParseReserve is the share of the machine's memory the reserve setting
// keeps free of gated runs: "15%", or 0.15.
func ParseReserve(v map[string]string) float64 {
	s, ok := v["reserve"]
	if !ok {
		s = Defaults["reserve"]
	}
	s = strings.TrimSpace(s)
	f, err := strconv.ParseFloat(strings.TrimSuffix(s, "%"), 64)
	if strings.HasSuffix(s, "%") || f > 1 {
		f /= 100
	}
	if err != nil || f < 0 || f >= 1 {
		return 0.15
	}
	return f
}

// ParseFloor is the floor setting in percent: the memory left at which
// the guard stops agents' programs; 0 when it's off.
func ParseFloor(v map[string]string) int {
	s, ok := v["floor"]
	if !ok {
		s = Defaults["floor"]
	}
	s = strings.TrimSpace(s)
	if s == "off" {
		return 0
	}
	n, err := strconv.Atoi(strings.TrimSuffix(s, "%"))
	if err != nil || n <= 0 || n >= 50 {
		return DefaultFloor
	}
	return n
}

// ParseSize is a size such as 6G, 512M or 1.5GiB in bytes, counted in
// 1024s. A bare number isn't a size.
func ParseSize(s string) (uint64, bool) {
	s = strings.ToUpper(strings.TrimSpace(s))
	s = strings.TrimSuffix(strings.TrimSuffix(s, "B"), "I")
	if s == "" {
		return 0, false
	}
	var mult float64
	switch s[len(s)-1] {
	case 'K':
		mult = 1 << 10
	case 'M':
		mult = 1 << 20
	case 'G':
		mult = 1 << 30
	case 'T':
		mult = 1 << 40
	default:
		return 0, false
	}
	f, err := strconv.ParseFloat(s[:len(s)-1], 64)
	if err != nil || f <= 0 {
		return 0, false
	}
	return uint64(f * mult), true
}

// Size is n bytes for people: 3.1 GiB, 640 MiB.
func Size(n uint64) string {
	if n >= gib {
		return fmt.Sprintf("%.1f GiB", float64(n)/gib)
	}
	return fmt.Sprintf("%d MiB", n>>20)
}

func fields(s string) []string {
	return strings.FieldsFunc(s, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' || r == '\n' })
}
