package convo

import (
	"regexp"
	"sync"
)

// lazyRe is a pattern compiled when first used rather than when rush
// starts: most of a process's life never draws a card that needs it.
func lazyRe(expr string) func() *regexp.Regexp {
	return sync.OnceValue(func() *regexp.Regexp { return regexp.MustCompile(expr) })
}
