//go:build !linux

package proc

// Subreap does nothing off Linux: macOS has no subreaper, so a process
// whose parent exits goes to launchd.
func Subreap() error { return nil }
