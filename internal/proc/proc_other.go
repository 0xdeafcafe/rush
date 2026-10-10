//go:build !darwin && !linux

package proc

import (
	"syscall"
	"time"
)

// Other systems have no process backend: the process columns stay empty.
func list() []*Proc                          { return nil }
func fillUsage(*Proc)                        {}
func args(int) []string                      { return nil }
func env(int) []string                       { return nil }
func CommandLine(int) string                 { return "" }
func Kill(pid int, sig syscall.Signal) error { return syscall.Kill(pid, sig) }
func Zombie(int) bool                        { return false }
func Started(int) (time.Time, bool)          { return time.Time{}, false }
