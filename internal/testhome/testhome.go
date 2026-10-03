// Package testhome keeps tests off the machine's own rush folders: every
// test package that reaches internal/state calls Main from its TestMain.
package testhome

import (
	"os"
	"testing"
)

const mark = "RUSH_TESTHOME"

// Main points RUSH_HOME and RUSH_CACHE at folders of the run's own, runs
// m and exits. A process the tests start again (a host, a plugin) gets
// the same folders from its environment, so it keeps them.
func Main(m *testing.M) {
	os.Exit(Run(m))
}

// Run is Main without the exit, for a TestMain with more to do around it.
func Run(m *testing.M) int {
	cleanup := Isolate()
	defer cleanup()
	return m.Run()
}

// Isolate sets RUSH_HOME and RUSH_CACHE to fresh folders, unless an
// enclosing test process already did, and returns what removes them.
func Isolate() func() {
	if os.Getenv(mark) != "" {
		return func() {}
	}
	d, err := os.MkdirTemp("", "rush-test")
	if err != nil {
		panic(err)
	}
	os.Setenv(mark, d)
	os.Setenv("RUSH_HOME", d+"/home")
	os.Setenv("RUSH_CACHE", d+"/cache")
	return func() { os.RemoveAll(d) }
}
