package fleet

import (
	"time"

	"golang.org/x/sys/unix"
)

// born is when p was made, where the file system says.
func born(p string) (time.Time, bool) {
	var st unix.Stat_t
	if unix.Lstat(p, &st) != nil {
		return time.Time{}, false
	}
	return time.Unix(st.Btim.Unix()), true
}
