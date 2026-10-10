package gate

import (
	"os"
	"strconv"
	"strings"
)

// load1 is the one-minute load average, or -1 when it can't be read.
func load1() float64 {
	b, err := os.ReadFile("/proc/loadavg")
	if err != nil {
		return -1
	}
	f := strings.Fields(string(b))
	if len(f) == 0 {
		return -1
	}
	v, err := strconv.ParseFloat(f[0], 64)
	if err != nil {
		return -1
	}
	return v
}
