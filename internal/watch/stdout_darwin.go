package watch

import (
	"bytes"
	"syscall"
	"unsafe"
)

// stdoutPath is the file stdout writes to, as fcntl's F_GETPATH says.
func stdoutPath() string {
	const fGetPath = 50 // F_GETPATH, <sys/fcntl.h>
	buf := make([]byte, 1024)
	if _, _, errno := syscall.Syscall(syscall.SYS_FCNTL, 1, fGetPath, uintptr(unsafe.Pointer(&buf[0]))); errno != 0 {
		return ""
	}
	if i := bytes.IndexByte(buf, 0); i >= 0 {
		buf = buf[:i]
	}
	return string(buf)
}
