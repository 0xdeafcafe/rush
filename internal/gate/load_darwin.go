package gate

import (
	"encoding/binary"

	"golang.org/x/sys/unix"
)

// load1 is the one-minute load average, or -1 when it can't be read.
func load1() float64 {
	// struct loadavg { fixpt_t ldavg[3]; long fscale; }
	b, err := unix.SysctlRaw("vm.loadavg")
	if err != nil || len(b) < 16 {
		return -1
	}
	scale := binary.LittleEndian.Uint64(b[len(b)-8:])
	if scale == 0 {
		return -1
	}
	return float64(binary.LittleEndian.Uint32(b[0:4])) / float64(scale)
}
