package gate

import (
	"os/exec"
	"strconv"
	"strings"

	"golang.org/x/sys/unix"
)

// memory is what the machine has, available and in all, in bytes, as
// vm_stat counts it: free, inactive and purgeable pages are available.
// 0, 0 when it can't be read.
func memory() (avail, total uint64) {
	total, err := unix.SysctlUint64("hw.memsize")
	if err != nil {
		return 0, 0
	}
	out, err := exec.Command("/usr/bin/vm_stat").Output()
	if err != nil {
		return 0, 0
	}
	avail, ok := parseVMStat(string(out))
	if !ok {
		return 0, 0
	}
	return avail, total
}

// parseVMStat is the available bytes vm_stat's output says.
func parseVMStat(s string) (uint64, bool) {
	lines := strings.Split(s, "\n")
	page := uint64(0)
	if i := strings.Index(lines[0], "page size of "); i >= 0 {
		if f := strings.Fields(lines[0][i+len("page size of "):]); len(f) > 0 {
			page, _ = strconv.ParseUint(f[0], 10, 64)
		}
	}
	if page == 0 {
		return 0, false
	}
	var pages uint64
	for _, l := range lines[1:] {
		k, v, ok := strings.Cut(l, ":")
		if !ok {
			continue
		}
		switch k {
		case "Pages free", "Pages inactive", "Pages purgeable":
			n, err := strconv.ParseUint(strings.TrimSuffix(strings.TrimSpace(v), "."), 10, 64)
			if err != nil {
				return 0, false
			}
			pages += n
		}
	}
	return pages * page, true
}

// pressure is kern.memorystatus_vm_pressure_level: 1 normal, 2 warning,
// 4 critical; 0 when it can't be read.
func pressure() int {
	v, err := unix.SysctlUint32("kern.memorystatus_vm_pressure_level")
	if err != nil {
		return 0
	}
	return int(v)
}

// memLevel is kern.memorystatus_level: the share of memory the kernel
// counts available, 0 to 100, as Activity Monitor's pressure graph has it;
// -1 when it can't be read. One sysctl, no fork, so it still reads when
// the machine is paging hard.
func memLevel() int {
	v, err := unix.SysctlUint32("kern.memorystatus_level")
	if err != nil || v > 100 {
		return -1
	}
	return int(v)
}

// renice lowers pid's priority to nice n, never raising it: one already
// nicer keeps its own.
func renice(pid, n int) {
	if cur, err := unix.Getpriority(unix.PRIO_PROCESS, pid); err == nil && cur >= n {
		return
	}
	_ = unix.Setpriority(unix.PRIO_PROCESS, pid, n)
}
