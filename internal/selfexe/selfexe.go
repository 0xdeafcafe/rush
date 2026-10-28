// Package selfexe is the running rush as a file to start again.
package selfexe

import (
	"fmt"
	"os"
	"time"
)

// Settle is how long Path waits for an install to finish writing rush.
var Settle = 3 * time.Second

// Path is os.Executable once the file there can be started. go install
// removes the old rush before it writes the new one, so a rush started
// in that gap fails with "no such file or directory", or starts half a
// file. Path waits out the gap: until the file is there and has stopped
// growing, for up to Settle.
func Path() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	return exe, Wait(exe)
}

// Wait blocks until the file at exe is there and not being written, for
// up to Settle. A file last written a while ago is settled at once.
func Wait(exe string) error {
	const tick = 50 * time.Millisecond
	deadline := time.Now().Add(Settle)
	var last os.FileInfo
	for {
		fi, err := os.Stat(exe)
		switch {
		case err == nil && time.Since(fi.ModTime()) > time.Second:
			return nil
		case err == nil && last != nil && fi.Size() == last.Size() && fi.ModTime().Equal(last.ModTime()):
			return nil
		}
		last = fi
		if time.Now().After(deadline) {
			if err != nil {
				return fmt.Errorf("rush isn't at %s (is it being installed?): %w", exe, err)
			}
			return nil
		}
		time.Sleep(tick)
	}
}
