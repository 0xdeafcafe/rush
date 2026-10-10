//go:build !darwin && !linux

package gate

// load1 is the one-minute load average, or -1 when it can't be read.
func load1() float64 { return -1 }
