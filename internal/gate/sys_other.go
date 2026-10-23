//go:build !darwin && !linux

package gate

func memory() (avail, total uint64) { return 0, 0 }
func pressure() int                 { return 0 }
func memLevel() int                 { return -1 }
func renice(int, int)               {}
