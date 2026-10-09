//go:build linux

package proc

import (
	"bytes"
	"os"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// clockTicks is USER_HZ, the unit of the times in /proc/<pid>/stat. It is
// 100 on every Linux architecture Go supports.
const clockTicks = 100

var pageSize = uint64(os.Getpagesize())

// bootTime is when the system started, the zero of a process's start time.
var bootTime = func() time.Time {
	b, err := os.ReadFile("/proc/stat")
	if err != nil {
		return time.Time{}
	}
	for _, line := range strings.Split(string(b), "\n") {
		if rest, ok := strings.CutPrefix(line, "btime "); ok {
			if s, err := strconv.ParseInt(strings.TrimSpace(rest), 10, 64); err == nil {
				return time.Unix(s, 0)
			}
		}
	}
	return time.Time{}
}()

// stat is one /proc/<pid>/stat: the command, the parent, the CPU time and
// the start time.
type stat struct {
	comm       string
	ppid       int
	cpu        time.Duration
	startTicks uint64
}

// parseStat reads the stat line. The command sits in parentheses and may
// hold spaces or parentheses itself, so the fields after it are counted
// from its last ')'.
func parseStat(b []byte) (stat, bool) {
	open, end := bytes.IndexByte(b, '('), bytes.LastIndexByte(b, ')')
	if open < 0 || end < open {
		return stat{}, false
	}
	f := strings.Fields(string(b[end+1:]))
	// f[0] is the state (field 3); ppid is field 4, utime 14, stime 15,
	// starttime 22.
	if len(f) < 20 {
		return stat{}, false
	}
	ppid, _ := strconv.Atoi(f[1])
	utime, _ := strconv.ParseUint(f[11], 10, 64)
	stime, _ := strconv.ParseUint(f[12], 10, 64)
	start, _ := strconv.ParseUint(f[19], 10, 64)
	return stat{
		comm:       string(b[open+1 : end]),
		ppid:       ppid,
		cpu:        time.Duration(utime+stime) * time.Second / clockTicks,
		startTicks: start,
	}, true
}

func readStat(pid int) (stat, bool) {
	b, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat")
	if err != nil {
		return stat{}, false
	}
	return parseStat(b)
}

func list() []*Proc {
	entries, err := os.ReadDir("/proc")
	if err != nil {
		return nil
	}
	out := make([]*Proc, 0, len(entries))
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil || pid <= 0 {
			continue
		}
		s, ok := readStat(pid)
		if !ok {
			continue // gone since the directory was read
		}
		p := &Proc{PID: pid, PPID: s.ppid, Comm: s.comm}
		if !bootTime.IsZero() {
			p.Start = bootTime.Add(time.Duration(s.startTicks) * time.Second / clockTicks)
		}
		out = append(out, p)
	}
	return out
}

// fillUsage reads the CPU time from stat and the resident memory from
// statm, the nearest Linux has to the footprint macOS reports.
func fillUsage(p *Proc) {
	if s, ok := readStat(p.PID); ok {
		p.CPUTime = s.cpu
	}
	b, err := os.ReadFile("/proc/" + strconv.Itoa(p.PID) + "/statm")
	if err != nil {
		return
	}
	f := strings.Fields(string(b))
	if len(f) < 2 {
		return
	}
	if rss, err := strconv.ParseUint(f[1], 10, 64); err == nil {
		p.Footprint = rss * pageSize
	}
}

// args reads a process's argv from /proc/<pid>/cmdline.
func args(pid int) []string {
	b, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/cmdline")
	if err != nil || len(b) == 0 {
		return nil
	}
	return strings.Split(strings.TrimRight(string(b), "\x00"), "\x00")
}

// env reads a process's environment from /proc/<pid>/environ.
func env(pid int) []string {
	b, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/environ")
	if err != nil || len(b) == 0 {
		return nil
	}
	return strings.Split(strings.TrimRight(string(b), "\x00"), "\x00")
}

func CommandLine(pid int) string { return strings.Join(Args(pid), " ") }

func Kill(pid int, sig syscall.Signal) error {
	if sig != 0 { // asking whether it runs harms nothing
		if err := guard(pid); err != nil {
			return err
		}
	}
	return syscall.Kill(pid, sig)
}

// Zombie is whether pid has exited but its parent hasn't reaped it: it
// still answers kill(pid, 0), though nothing runs.
func Zombie(pid int) bool {
	b, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat")
	if err != nil {
		return false
	}
	end := bytes.LastIndexByte(b, ')')
	return end >= 0 && end+2 < len(b) && b[end+2] == 'Z'
}
