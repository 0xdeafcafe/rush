//go:build !darwin

package watch

import "os"

// stdoutPath is the file stdout writes to, as /proc says.
func stdoutPath() string {
	p, _ := os.Readlink("/proc/self/fd/1")
	return p
}
