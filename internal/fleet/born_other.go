//go:build !darwin

package fleet

import "time"

// born is when p was made: not known here, so nothing passes for an
// agent's own without asking.
func born(string) (time.Time, bool) { return time.Time{}, false }
