package gate

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"golang.org/x/sys/unix"
)

// memory is what the machine has, available and in all, in bytes, as
// /proc/meminfo says; 0, 0 when it can't be read.
func memory() (avail, total uint64) {
	b, err := os.ReadFile("/proc/meminfo")
	if err != nil {
		return 0, 0
	}
	for _, l := range strings.Split(string(b), "\n") {
		f := strings.Fields(l)
		if len(f) < 2 {
			continue
		}
		n, _ := strconv.ParseUint(f[1], 10, 64)
		switch f[0] {
		case "MemAvailable:":
			avail = n << 10
		case "MemTotal:":
			total = n << 10
		}
	}
	return avail, total
}

// pressure is unknown here: no run is paused for it.
func pressure() int { return 0 }

// renice lowers each of pid's threads to nice n, never raising one: Linux
// keeps a nice a thread.
func renice(pid, n int) {
	tasks, _ := filepath.Glob(filepath.Join("/proc", strconv.Itoa(pid), "task", "*"))
	for _, t := range tasks {
		tid, err := strconv.Atoi(filepath.Base(t))
		if err != nil {
			continue
		}
		// The raw call is 20 less the nice, so it's never negative.
		if raw, err := unix.Getpriority(unix.PRIO_PROCESS, tid); err == nil && 20-raw >= n {
			continue
		}
		_ = unix.Setpriority(unix.PRIO_PROCESS, tid, n)
	}
}
